//! A generic importer for markdown knowledge bases with YAML frontmatter.
//!
//! [`import_tree`] walks a source directory and converts every `.md` file
//! into canonical Engram shape: it ensures `type` is set (applying a legacy
//! type mapping table and tagging the original value), backfills missing
//! temporal metadata, drops sentinel open-ended dates, strips a leading
//! source-tree permalink prefix and adds missing write provenance
//! (`generated`). Every other
//! frontmatter key and the body are preserved exactly (written with LF line
//! endings, the one line ending Crystalline stores): each file is read with
//! [`crate::parse::parse_engram_lossless`], the typed frontmatter is edited in
//! place and the result is written back with [`crate::emit::emit_engram`],
//! which keeps unknown keys in their original order and the body verbatim.
//! Non-markdown files are copied byte for byte.
//!
//! This is a pure file transformation: no database, network or config
//! registry lookup happens here, so the derived index never needs to exist
//! for an import to run. Idempotency is a property of the rules themselves,
//! not a marker: once a file carries an explicit `type`, temporal metadata, no
//! sentinel dates, an already-stripped permalink and write provenance, running
//! the importer again on it is a no-op.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::emit::emit_engram;
use crate::engram::{Engram, Generated};
use crate::linked::{DirSource, LinkedNote, LinkedSource, NotTaken, take_linked_files};
use crate::parse::{ParseError, parse_engram, parse_engram_lossless};
use crate::time::{self, Day, OffsetStamp, Stamp};

/// The OKF actor the importer records as the writer of the engrams it
/// normalizes: an automated job, per the spec's `process:name` convention.
pub const IMPORT_ACTOR: &str = "process:crystalline-import";

/// The year at or above which a `valid_to` date is treated as the sentinel
/// "valid forever" convention some source knowledge bases write literally
/// (for example `9999-12-31`), rather than Crystalline's own convention of
/// simply omitting the field. Matches `verify`'s `T008` threshold.
pub const SENTINEL_FUTURE_YEAR: i32 = 9000;

/// The year at or below which a `valid_from` date is treated as the sentinel
/// "has always been valid" convention (for example `0001-01-01`), the
/// past-facing counterpart to [`SENTINEL_FUTURE_YEAR`].
pub const SENTINEL_PAST_YEAR: i32 = 1;

/// The `type` used when a source file has no `type` at all.
pub const DEFAULT_TYPE: &str = "engram";

/// Built-in legacy `type` to Crystalline `type` mappings, applied when a
/// source file's `type` exactly matches one of these values. Overridable and
/// extensible per import via `--map`.
pub const DEFAULT_TYPE_MAP: &[(&str, &str)] = &[
    ("changelog", "reference"),
    ("howto", "guide"),
    ("pattern", "engram"),
    ("poc", "engram"),
    ("security", "engram"),
    ("class", "reference"),
    ("note", "engram"),
];

/// Build the built-in legacy type mapping as an [`IndexMap`].
pub fn default_type_map() -> IndexMap<String, String> {
    DEFAULT_TYPE_MAP
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The shape of a `--map` override file: `{ mappings: { old: new, ... } }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TypeMapFile {
    /// Legacy type to Crystalline type overrides, merged onto the built-in
    /// defaults. A caller may add new entries or replace a default one.
    #[serde(default)]
    pub mappings: IndexMap<String, String>,
}

/// Merge a `--map` override onto the built-in defaults. Override entries win.
pub fn merge_type_map(overrides: &IndexMap<String, String>) -> IndexMap<String, String> {
    let mut map = default_type_map();
    for (k, v) in overrides {
        map.insert(k.clone(), v.clone());
    }
    map
}

/// Options for one [`import_tree`] run.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// The source directory to walk.
    pub src_dir: PathBuf,
    /// The target domain's root directory. Relative paths from `src_dir` are
    /// preserved underneath it.
    pub domain_dir: PathBuf,
    /// Legacy `type` to Crystalline `type` mapping to apply.
    pub type_map: IndexMap<String, String>,
    /// The permalink prefix segment to strip. Defaults to `src_dir`'s final
    /// path component when `None`.
    pub strip_prefix: Option<String>,
    /// When set, nothing is written; the report describes what would happen.
    pub dry_run: bool,
    /// The `status` given to a file that has none.
    pub missing_status: MissingStatus,
}

/// The `status` an import gives a file that carries none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MissingStatus {
    /// `stable`: the file is taken as it is, and its `temporal_confidence`
    /// is kept where it has a valid one.
    #[default]
    Stable,
    /// `legacy` with `temporal_confidence: inferred`, the rule before 0.29.0.
    Legacy,
}

/// The tag an import adds to a file that has no tags at all.
pub const IMPORTED_TAG: &str = "imported";

/// What an importer did with one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileAction {
    /// A markdown file was parsed, transformed (possibly a no-op) and written.
    Converted,
    /// A non-markdown file was copied verbatim.
    Copied,
    /// A file could not be processed and was left alone.
    Skipped,
}

/// One file's outcome: what happened, and a human description of each
/// transform applied, in order. Empty `changes` on a `Converted` file means
/// the file already matched canonical shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    /// The path, relative to `src_dir`, forward-slashed.
    pub path: String,
    /// What happened to the file.
    pub action: FileAction,
    /// Human-readable descriptions of each transform applied, in order.
    pub changes: Vec<String>,
}

/// Aggregate counts and per-file detail for one [`import_tree`] run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    /// Markdown files parsed and (re)written.
    pub files_converted: usize,
    /// Non-markdown files copied verbatim.
    pub files_copied: usize,
    /// Files that could not be processed.
    pub files_skipped: usize,
    /// Distinct files notes link to that were taken into `assets/`, files the
    /// domain already held included.
    pub files_linked: usize,
    /// Non-markdown files no note links, copied verbatim to their own path.
    pub not_linked: usize,
    /// Links to local files that were not taken, with the reason.
    pub not_taken: Vec<NotTaken>,
    /// Files whose `type` was set or remapped.
    pub type_mapped: usize,
    /// Temporal fields backfilled (each of `status` and `recorded_at` counts
    /// separately, so one file can contribute up to two).
    pub temporal_backfilled: usize,
    /// Sentinel `valid_from`/`valid_to` dates dropped.
    pub sentinels_dropped: usize,
    /// Permalinks whose source-tree prefix was stripped.
    pub prefixes_stripped: usize,
    /// Permalink collisions resolved with an `-imported-N` suffix.
    pub collisions: usize,
    /// Non-fatal warnings, for example an unreadable file or a resolved
    /// collision.
    pub warnings: Vec<String>,
    /// Per-file detail, in the order files were processed (sorted by path).
    pub files: Vec<FileChange>,
}

/// An error from [`import_tree`] itself, as opposed to a per-file problem
/// (which is reported as a skipped file with a warning).
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The source directory does not exist or is not a directory.
    #[error("source directory does not exist: {}", .0.display())]
    SourceNotFound(PathBuf),
    /// An IO error walking, reading or writing a file.
    #[error("io error at {}: {source}", .path.display())]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The underlying IO error.
        source: std::io::Error,
    },
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> ImportError + '_ {
    move |source| ImportError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Import every file under `options.src_dir` into `options.domain_dir`.
/// Where a Reflection keeps `reflections/` (the target domain's MANIFEST or
/// the source tree's own declares `reflection: on`), every file there is
/// skipped with [`crate::reflection::REFLECTION_NOT_IMPORTED`] as its
/// warning.
pub fn import_tree(options: &ImportOptions) -> Result<ImportReport, ImportError> {
    if !options.src_dir.is_dir() {
        return Err(ImportError::SourceNotFound(options.src_dir.clone()));
    }
    let strip_prefix = options.strip_prefix.clone().unwrap_or_else(|| {
        options
            .src_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });

    let entries = collect_entries(&options.src_dir)?;
    let batch_rels: HashSet<String> = entries
        .iter()
        .filter(|e| e.is_md)
        .map(|e| e.rel.clone())
        .collect();
    let mut permalinks = existing_domain_permalinks(&options.domain_dir, &batch_rels);
    let now = time::now().to_offset_utc();

    let mut report = ImportReport::default();

    // R33, as on every other import road: where a Reflection keeps
    // `reflections/` (the target domain declares the switch on, pending
    // included, or the source tree's own MANIFEST does), nothing there is
    // converted into an engram or copied into the folder the Reflection
    // owns. With both off it is an ordinary folder, as before.
    let held = crate::manifest::reflection_at(&options.domain_dir)
        == crate::manifest::ReflectionSwitch::On
        || crate::manifest::reflection_at(&options.src_dir)
            == crate::manifest::ReflectionSwitch::On;
    let layout = crate::layout::DomainLayout::bare(if held {
        crate::layout::ReflectionMode::Effective
    } else {
        crate::layout::ReflectionMode::Off
    });

    // Phase 1: complete every note, so the linked-file step sees them all.
    let mut planned: Vec<Planned> = Vec::new();
    for entry in &entries {
        if matches!(
            layout.classify(&entry.rel),
            crate::layout::PathClass::Reflection(_)
        ) {
            report.files_skipped += 1;
            report.warnings.push(format!(
                "{}: {}",
                entry.rel,
                crate::reflection::REFLECTION_NOT_IMPORTED
            ));
            planned.push(Planned::Skipped(skipped_change(&entry.rel)));
            continue;
        }
        if !entry.is_md {
            planned.push(Planned::Verbatim(entry));
            continue;
        }
        planned.push(prepare_markdown(
            entry,
            options,
            &strip_prefix,
            now,
            &mut permalinks,
            &mut report,
        ));
    }

    // Phase 2: take the files the notes link to and rewrite the links.
    let mut notes: Vec<LinkedNote> = planned
        .iter()
        .filter_map(|p| match p {
            Planned::Markdown(m) => Some(LinkedNote {
                rel: m.rel.clone(),
                text: m.text.clone(),
            }),
            _ => None,
        })
        .collect();
    let domain_files = DirSource {
        root: options.domain_dir.clone(),
    };
    let outcome = take_linked_files(
        &mut notes,
        &DirSource {
            root: options.src_dir.clone(),
        },
        &|dest| domain_files.sha256(dest).ok(),
    );
    let mut rewritten = notes.into_iter();

    // Phase 3: copy the taken files first, so a failed copy leaves no written
    // note that links a missing asset; then write the notes, copy the rest.
    for taken in &outcome.taken {
        if taken.reused || options.dry_run {
            continue;
        }
        let from = options.src_dir.join(&taken.source_rel);
        let dest = options.domain_dir.join(&taken.dest);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        crate::config::copy_atomic(&from, &dest).map_err(io_err(&from))?;
    }
    for item in planned {
        match item {
            Planned::Skipped(change) => report.files.push(change),
            Planned::Verbatim(entry) => {
                if outcome.linked_sources.contains(&entry.rel) {
                    continue;
                }
                copy_verbatim(entry, options, &mut report)?;
                report.not_linked += 1;
            }
            Planned::Markdown(m) => {
                let note = rewritten.next().expect("one rewritten note per note");
                write_markdown(*m, note.text, options, &mut report)?;
            }
        }
    }
    report.files_linked = outcome.taken.len();
    report.not_taken = outcome.not_taken;

    Ok(report)
}

/// One source entry after phase 1.
enum Planned<'a> {
    /// Already counted and warned about; only its report line is left.
    Skipped(FileChange),
    /// A non-markdown file, copied as it is unless a note links it.
    Verbatim(&'a Entry),
    /// A completed note waiting for its links to be settled.
    Markdown(Box<PreparedNote>),
}

/// A note completed in memory.
struct PreparedNote {
    rel: String,
    /// The emitted text, LF line endings.
    text: String,
    changes: Vec<String>,
    type_mapped: bool,
    temporal_backfilled: usize,
    sentinels_dropped: usize,
    prefix_stripped: bool,
}

fn skipped_change(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        action: FileAction::Skipped,
        changes: Vec::new(),
    }
}

fn copy_verbatim(
    entry: &Entry,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let dest = options.domain_dir.join(&entry.rel);
    if !options.dry_run {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        // Copying a file onto itself truncates on some platforms and is a
        // sharing violation on Windows, so a re-import where source and
        // target coincide must leave the file alone.
        let same_file = dest.exists()
            && std::fs::canonicalize(&entry.abs).ok() == std::fs::canonicalize(&dest).ok();
        if !same_file {
            // Copy-then-rename (crate::config::copy_atomic), not a direct
            // copy: a crash mid-copy can no longer leave a truncated file at
            // `dest` for a reader (or a re-import) to trip over.
            crate::config::copy_atomic(&entry.abs, &dest).map_err(io_err(&entry.abs))?;
        }
    }
    report.files_copied += 1;
    report.files.push(FileChange {
        path: entry.rel.clone(),
        action: FileAction::Copied,
        changes: Vec::new(),
    });
    Ok(())
}

fn prepare_markdown<'a>(
    entry: &'a Entry,
    options: &ImportOptions,
    strip_prefix: &str,
    now: OffsetStamp,
    permalinks: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> Planned<'a> {
    let source = match std::fs::read_to_string(&entry.abs) {
        Ok(s) => s,
        Err(e) => {
            report.files_skipped += 1;
            report
                .warnings
                .push(format!("{}: could not read file: {e}", entry.rel));
            return Planned::Skipped(skipped_change(&entry.rel));
        }
    };

    let complete_opts = CompleteOptions {
        rel_path: &entry.rel,
        type_map: &options.type_map,
        strip_prefix,
        mtime: mtime_date(&entry.abs),
        now,
        missing_status: options.missing_status,
    };
    let mut transformed = match complete_engram(&source, &complete_opts) {
        Ok(t) => t,
        Err(e) => {
            report.files_skipped += 1;
            report.warnings.push(format!("{}: {e}", entry.rel));
            return Planned::Skipped(skipped_change(&entry.rel));
        }
    };

    if let Some(candidate) = transformed.engram.frontmatter.permalink.clone() {
        let final_permalink = resolve_collision(&candidate, &entry.rel, permalinks, report);
        if final_permalink != candidate {
            transformed.engram.frontmatter.permalink = Some(final_permalink.clone());
            transformed.changes.push(format!(
                "permalink: `{candidate}` collided, renamed to `{final_permalink}`"
            ));
        }
        permalinks.insert(final_permalink, entry.rel.clone());
    }

    // Written as LF, the one line ending Crystalline stores: the body is
    // carried over verbatim apart from its line endings.
    let text = crate::to_lf(&emit_engram(&transformed.engram)).into_owned();
    Planned::Markdown(Box::new(PreparedNote {
        rel: entry.rel.clone(),
        text,
        changes: transformed.changes,
        type_mapped: transformed.type_mapped,
        temporal_backfilled: transformed.temporal_backfilled,
        sentinels_dropped: transformed.sentinels_dropped,
        prefix_stripped: transformed.prefix_stripped,
    }))
}

fn write_markdown(
    note: PreparedNote,
    text: String,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    if !options.dry_run {
        let dest = options.domain_dir.join(&note.rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        // Write-then-rename (crate::config::write_atomic), not a direct
        // write: a crash mid-write can no longer leave a truncated engram
        // file at `dest`.
        crate::config::write_atomic(&dest, text.as_bytes()).map_err(io_err(&dest))?;
    }

    report.files_converted += 1;
    if note.type_mapped {
        report.type_mapped += 1;
    }
    report.temporal_backfilled += note.temporal_backfilled;
    report.sentinels_dropped += note.sentinels_dropped;
    if note.prefix_stripped {
        report.prefixes_stripped += 1;
    }
    report.files.push(FileChange {
        path: note.rel,
        action: FileAction::Converted,
        changes: note.changes,
    });
    Ok(())
}

fn resolve_collision(
    candidate: &str,
    owner_rel: &str,
    permalinks: &HashMap<String, String>,
    report: &mut ImportReport,
) -> String {
    if !permalinks.contains_key(candidate) {
        return candidate.to_string();
    }
    let mut n = 1usize;
    loop {
        let attempt = format!("{candidate}-imported-{n}");
        if !permalinks.contains_key(&attempt) {
            report.collisions += 1;
            report.warnings.push(format!(
                "{owner_rel}: permalink `{candidate}` collides with `{}`, renamed to `{attempt}`",
                permalinks.get(candidate).map(String::as_str).unwrap_or("?")
            ));
            return attempt;
        }
        n += 1;
    }
}

/// What [`complete_engram`] needs besides the file's text.
#[derive(Debug, Clone)]
pub struct CompleteOptions<'a> {
    /// The file's path relative to the domain root, forward-slashed, with its
    /// `.md`. A missing permalink is derived from it the way sync does.
    pub rel_path: &'a str,
    /// Legacy `type` to Crystalline `type` mapping.
    pub type_map: &'a IndexMap<String, String>,
    /// The leading permalink segment to strip.
    pub strip_prefix: &'a str,
    /// The file's modification date, used for a missing `recorded_at`.
    pub mtime: Option<Day>,
    /// The time recorded as the write time of the provenance.
    pub now: OffsetStamp,
    /// The `status` given to a file that has none.
    pub missing_status: MissingStatus,
}

/// The result of completing one Engram's frontmatter. `engram` is ready to
/// pass to [`emit_engram`] once any permalink collision is resolved by the
/// caller.
#[derive(Debug, Clone)]
pub struct Completed {
    /// The completed Engram.
    pub engram: Engram,
    /// A human description of each change, in order.
    pub changes: Vec<String>,
    /// `type` was set or remapped.
    pub type_mapped: bool,
    /// Temporal fields backfilled (`status` and `recorded_at` count apart).
    pub temporal_backfilled: usize,
    /// Sentinel dates dropped.
    pub sentinels_dropped: usize,
    /// A source-tree permalink prefix was stripped.
    pub prefix_stripped: bool,
    /// A missing permalink was derived from the path.
    pub permalink_backfilled: bool,
    /// The file had no tags and got [`IMPORTED_TAG`].
    pub tagged_imported: bool,
}

/// Complete one Engram's frontmatter so OKF readers accept it: every
/// per-file rule shared by all import routes. Pure function, no filesystem
/// access, so it is exercised directly in unit tests.
pub fn complete_engram(source: &str, opts: &CompleteOptions) -> Result<Completed, ParseError> {
    let type_map = opts.type_map;
    let strip_prefix = opts.strip_prefix;
    let mtime = opts.mtime;
    let now = opts.now;
    let lossless = parse_engram_lossless(source)?;
    let mut engram = lossless.engram;
    let mut changes = Vec::new();
    let mut type_mapped = false;
    let mut temporal_backfilled = 0usize;
    let mut sentinels_dropped = 0usize;
    let mut prefix_stripped = false;
    let mut permalink_backfilled = false;
    let mut tagged_imported = false;

    // a. Ensure `type`: missing becomes the default; a legacy value is
    // remapped and the original value is kept as a deduped tag.
    let fm = &mut engram.frontmatter;
    if fm.engram_type.trim().is_empty() {
        fm.engram_type = DEFAULT_TYPE.to_string();
        type_mapped = true;
        changes.push(format!("type: missing -> `{DEFAULT_TYPE}`"));
    } else if let Some(mapped) = type_map.get(fm.engram_type.as_str())
        && mapped != &fm.engram_type
    {
        let old = fm.engram_type.clone();
        fm.engram_type = mapped.clone();
        type_mapped = true;
        if !fm.tags.iter().any(|t| t == &old) {
            fm.tags.push(old.clone());
        }
        changes.push(format!("type: `{old}` -> `{mapped}` (tagged `{old}`)"));
    }

    // b. Temporal backfill only where absent.
    if fm.status.as_deref().map(str::trim).unwrap_or("").is_empty() {
        match opts.missing_status {
            MissingStatus::Stable => {
                fm.status = Some("stable".to_string());
                let kept = fm
                    .temporal_confidence
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| matches!(*c, "explicit" | "inferred"));
                match kept {
                    Some(c) => changes.push(format!(
                        "status: missing -> `stable` (temporal_confidence: {c} kept)"
                    )),
                    None => {
                        fm.temporal_confidence = Some("inferred".to_string());
                        changes.push(
                            "status: missing -> `stable` (temporal_confidence: inferred)"
                                .to_string(),
                        );
                    }
                }
            }
            MissingStatus::Legacy => {
                fm.status = Some("legacy".to_string());
                fm.temporal_confidence = Some("inferred".to_string());
                changes.push(
                    "status: missing -> `legacy` (temporal_confidence: inferred)".to_string(),
                );
            }
        }
        temporal_backfilled += 1;
    }
    if fm.recorded_at.is_none() {
        // The file's own date where there is one, else the day of the import:
        // no route can leave `recorded_at` out by passing no mtime.
        let (date, from) = match mtime {
            Some(date) => (date, "file mtime"),
            None => (now.day(), "import date"),
        };
        fm.recorded_at = Some(date);
        temporal_backfilled += 1;
        changes.push(format!("recorded_at: missing -> `{}` ({from})", date.ymd()));
    }

    // c. Drop sentinel far-future `valid_to` and far-past `valid_from`
    // dates: absence is what expresses open-ended validity.
    if let Some(to) = fm.valid_to
        && to.year() >= SENTINEL_FUTURE_YEAR
    {
        fm.valid_to = None;
        sentinels_dropped += 1;
        changes.push(format!("valid_to: dropped sentinel `{}`", to.ymd()));
    }
    if let Some(from) = fm.valid_from
        && from.year() <= SENTINEL_PAST_YEAR
    {
        fm.valid_from = None;
        sentinels_dropped += 1;
        changes.push(format!("valid_from: dropped sentinel `{}`", from.ymd()));
    }

    // d. Strip a leading source-prefix permalink segment.
    if let Some(p) = &fm.permalink
        && let Some((first, rest)) = p.split_once('/')
        && first == strip_prefix
        && !rest.is_empty()
    {
        let stripped = rest.to_string();
        changes.push(format!(
            "permalink: stripped prefix `{strip_prefix}/` -> `{stripped}`"
        ));
        fm.permalink = Some(stripped);
        prefix_stripped = true;
    }

    // d2. A permalink still missing is written the way sync derives it.
    if fm.permalink.as_deref().is_none_or(str::is_empty) {
        let derived = crate::slugify(opts.rel_path);
        if derived.is_empty() {
            // A name with no ASCII letter or digit slugifies to nothing, and
            // an empty permalink is worse than none: leave it missing.
            changes.push(format!(
                "warning: no permalink could be derived from `{}`, none written",
                opts.rel_path
            ));
        } else {
            changes.push(format!("permalink: missing -> `{derived}`"));
            fm.permalink = Some(derived);
            permalink_backfilled = true;
        }
    }

    // e. Add write provenance only when the file carries none. A file that
    // still has the legacy `timestamp` key is left alone: it already records
    // when it was written, and its next edit migrates it to `generated`.
    if fm.generated.is_none() && fm.timestamp.is_none() {
        fm.generated = Some(Generated {
            by: IMPORT_ACTOR.to_string(),
            // An import writes no words of its own, so it reports no model.
            model: None,
            at: Some(now),
        });
        changes.push(format!(
            "generated: missing -> `{{ by: {IMPORT_ACTOR}, at: {} }}`",
            now.rfc3339()
        ));
    }

    // f. Tags: `parse_engram_lossless` already normalizes a comma-separated
    // string into a list. A file with none gets the `imported` tag; a file
    // with tags is never changed.
    if fm.tags.is_empty() {
        fm.tags.push(IMPORTED_TAG.to_string());
        tagged_imported = true;
        changes.push(format!("tags: none -> `{IMPORTED_TAG}`"));
    }

    Ok(Completed {
        engram,
        changes,
        type_mapped,
        temporal_backfilled,
        sentinels_dropped,
        prefix_stripped,
        permalink_backfilled,
        tagged_imported,
    })
}

/// The text `domain import` and the archive upload store for one note: the
/// note completed by [`complete_engram`] with no type map (a `type` is free
/// form, so a present one is never remapped; only a missing one is set), no
/// prefix to strip and a missing status read as `stable`, or `None` when the
/// note needs nothing and is kept byte for byte.
///
/// Both routes take files that are often Crystalline's own (an export, a
/// backup), so a file that already carries everything a reader requires is
/// not rewritten: adding write provenance alone, or a permalink that could
/// not be derived, is no reason to change it. With no file date at hand a
/// missing `recorded_at` becomes the day of `now`.
pub fn complete_for_import(
    source: &str,
    rel_path: &str,
    now: OffsetStamp,
) -> Result<Option<String>, ParseError> {
    let type_map = IndexMap::new();
    let completed = complete_engram(
        source,
        &CompleteOptions {
            rel_path,
            type_map: &type_map,
            strip_prefix: "",
            mtime: None,
            now,
            missing_status: MissingStatus::Stable,
        },
    )?;
    let needed = completed.type_mapped
        || completed.temporal_backfilled > 0
        || completed.sentinels_dropped > 0
        || completed.prefix_stripped
        || completed.permalink_backfilled
        || completed.tagged_imported;
    Ok(needed.then(|| crate::to_lf(&emit_engram(&completed.engram)).into_owned()))
}

fn mtime_date(path: &Path) -> Option<Day> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some(Stamp::from_system_time(modified)?.day_utc())
}

// --- filesystem walking -------------------------------------------------------

struct Entry {
    /// Path relative to the walked root, forward-slashed.
    rel: String,
    /// The absolute path.
    abs: PathBuf,
    /// Whether the file has a `.md` extension (case-insensitive).
    is_md: bool,
}

/// Walk `root`, skipping dot-directories (but not dot-files), returning every
/// regular file sorted by relative path for deterministic processing order.
fn collect_entries(root: &Path) -> Result<Vec<Entry>, ImportError> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_dot_dir(e));
    for entry in walker {
        let entry = entry.map_err(|e| ImportError::Io {
            path: e
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf()),
            source: e
                .into_io_error()
                .unwrap_or_else(|| std::io::Error::other("directory walk failed")),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = rel_path(root, entry.path());
        let is_md = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false);
        out.push(Entry {
            rel,
            abs: entry.path().to_path_buf(),
            is_md,
        });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn is_dot_dir(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir()
        && entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with('.'))
            .unwrap_or(false)
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Permalinks already present in the target domain, keyed by permalink to the
/// owning relative path (used only for collision warning messages). Files
/// whose relative path is also present in this import batch are excluded, so
/// re-importing a file onto itself is never reported as a collision with
/// itself.
fn existing_domain_permalinks(
    domain_dir: &Path,
    exclude_rels: &HashSet<String>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !domain_dir.is_dir() {
        return map;
    }
    let Ok(entries) = collect_entries(domain_dir) else {
        return map;
    };
    for entry in entries {
        if !entry.is_md || exclude_rels.contains(&entry.rel) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&entry.abs) else {
            continue;
        };
        let Ok(parsed) = parse_engram(&content) else {
            continue;
        };
        if let Some(p) = parsed.frontmatter.permalink.filter(|p| !p.is_empty()) {
            map.insert(p, entry.rel.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> OffsetStamp {
        time::parse_rfc3339("2026-07-02T00:00:00+00:00").unwrap()
    }

    fn opts<'a>(
        map: &'a IndexMap<String, String>,
        status: MissingStatus,
        mtime: Option<Day>,
    ) -> CompleteOptions<'a> {
        CompleteOptions {
            rel_path: "Notes/My Pump.md",
            type_map: map,
            strip_prefix: "legacy-kb",
            mtime,
            now: now(),
            missing_status: status,
        }
    }

    fn t(source: &str) -> Completed {
        complete_engram(
            source,
            &opts(&default_type_map(), MissingStatus::Stable, None),
        )
        .unwrap()
    }

    fn t_legacy(source: &str) -> Completed {
        complete_engram(
            source,
            &opts(&default_type_map(), MissingStatus::Legacy, None),
        )
        .unwrap()
    }

    #[test]
    fn missing_type_becomes_engram_without_a_tag() {
        let r = t(
            "---\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.engram_type, "engram");
        assert!(!r.engram.frontmatter.tags.contains(&"engram".to_string()));
        assert!(r.type_mapped);
    }

    #[test]
    fn legacy_type_is_mapped_and_tagged() {
        let r = t(
            "---\ntype: howto\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.engram_type, "guide");
        assert!(r.engram.frontmatter.tags.contains(&"howto".to_string()));
        assert!(r.type_mapped);
    }

    #[test]
    fn legacy_tag_is_deduped_when_already_present() {
        let r = t(
            "---\ntype: note\ntitle: X\ntags:\n  - note\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(
            r.engram
                .frontmatter
                .tags
                .iter()
                .filter(|t| *t == "note")
                .count(),
            1
        );
    }

    #[test]
    fn modern_type_is_left_alone() {
        let r = t(
            "---\ntype: guide\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.engram_type, "guide");
        assert!(!r.type_mapped);
        assert!(!r.changes.iter().any(|c| c.starts_with("type:")));
    }

    #[test]
    fn missing_status_becomes_stable_and_keeps_temporal_confidence() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\ntemporal_confidence: explicit\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.status.as_deref(), Some("stable"));
        assert_eq!(
            r.engram.frontmatter.temporal_confidence.as_deref(),
            Some("explicit")
        );
        assert_eq!(r.temporal_backfilled, 1);
    }

    #[test]
    fn missing_status_and_confidence_give_stable_and_inferred() {
        let r =
            t("---\ntype: engram\ntitle: X\ntags:\n  - t\nrecorded_at: 2026-01-01\n---\n\nbody\n");
        assert_eq!(r.engram.frontmatter.status.as_deref(), Some("stable"));
        assert_eq!(
            r.engram.frontmatter.temporal_confidence.as_deref(),
            Some("inferred")
        );
        assert_eq!(r.temporal_backfilled, 1);
    }

    #[test]
    fn legacy_on_request_keeps_the_old_rule() {
        let r = t_legacy(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\ntemporal_confidence: explicit\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.status.as_deref(), Some("legacy"));
        assert_eq!(
            r.engram.frontmatter.temporal_confidence.as_deref(),
            Some("inferred")
        );
    }

    #[test]
    fn present_status_is_untouched() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.status.as_deref(), Some("current"));
        assert_eq!(r.temporal_backfilled, 0);
    }

    #[test]
    fn missing_recorded_at_backfills_from_mtime() {
        let date = Day::from_ymd(2026, 3, 4).unwrap();
        let r = complete_engram(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\n---\n\nbody\n",
            &opts(&default_type_map(), MissingStatus::Stable, Some(date)),
        )
        .unwrap();
        assert_eq!(r.engram.frontmatter.recorded_at, Some(date));
        assert_eq!(r.temporal_backfilled, 1);
    }

    #[test]
    fn sentinel_valid_to_is_dropped() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\nvalid_to: 9999-12-31\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.valid_to, None);
        assert_eq!(r.sentinels_dropped, 1);
    }

    #[test]
    fn sentinel_valid_from_is_dropped() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\nvalid_from: 0001-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.valid_from, None);
        assert_eq!(r.sentinels_dropped, 1);
    }

    #[test]
    fn non_sentinel_valid_range_is_untouched() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\nvalid_from: 2026-01-01\nvalid_to: 2027-01-01\n---\n\nbody\n",
        );
        assert!(r.engram.frontmatter.valid_from.is_some());
        assert!(r.engram.frontmatter.valid_to.is_some());
        assert_eq!(r.sentinels_dropped, 0);
    }

    #[test]
    fn source_prefix_is_stripped_from_permalink() {
        let r = t(
            "---\ntype: engram\ntitle: X\npermalink: legacy-kb/guides/x\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.permalink.as_deref(), Some("guides/x"));
        assert!(r.prefix_stripped);
    }

    #[test]
    fn permalink_without_matching_prefix_is_untouched() {
        let r = t(
            "---\ntype: engram\ntitle: X\npermalink: guides/x\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.permalink.as_deref(), Some("guides/x"));
        assert!(!r.prefix_stripped);
    }

    #[test]
    fn single_segment_result_is_kept_as_is() {
        let r = t(
            "---\ntype: engram\ntitle: X\npermalink: legacy-kb/manifest\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.permalink.as_deref(), Some("manifest"));
    }

    #[test]
    fn missing_permalink_is_backfilled_as_sync_derives_it() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        // Sync derives `slugify(path)` from the domain-relative path.
        assert_eq!(
            r.engram.frontmatter.permalink.as_deref(),
            Some(crate::slugify("Notes/My Pump.md").as_str())
        );
        assert_eq!(
            r.engram.frontmatter.permalink.as_deref(),
            Some("notes/my-pump")
        );
        assert!(r.permalink_backfilled);
        assert!(
            r.changes
                .iter()
                .any(|c| c.starts_with("permalink: missing"))
        );
    }

    #[test]
    fn a_file_without_tags_gets_the_imported_tag() {
        let r = t(
            "---\ntype: engram\ntitle: X\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.tags, vec![IMPORTED_TAG.to_string()]);
        assert!(r.tagged_imported);
        assert!(r.changes.iter().any(|c| c.starts_with("tags:")));
    }

    #[test]
    fn existing_tags_are_never_changed() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - a\n  - b\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(
            r.engram.frontmatter.tags,
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(!r.tagged_imported);
    }

    #[test]
    fn a_completed_file_passes_verify() {
        let date = Day::from_ymd(2026, 3, 4);
        let r = complete_engram(
            "---\ntype: document\ntitle: Pump\n---\n\nbody text\n",
            &opts(&default_type_map(), MissingStatus::Stable, date),
        )
        .unwrap();
        let out = emit_engram(&r.engram);
        let issues = crate::verify::check_document("d", Path::new("Notes/My Pump.md"), &out);
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == crate::verify::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn generated_is_added_only_when_the_file_carries_no_provenance() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        let g = r.engram.frontmatter.generated.as_ref().unwrap();
        assert_eq!(g.by, IMPORT_ACTOR);
        assert!(g.at.is_some());
        assert!(r.changes.iter().any(|c| c.starts_with("generated")));

        // A legacy `timestamp` already records when the file was written, so
        // the import leaves it alone; its next edit migrates it.
        let r2 = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2020-01-01T00:00:00+00:00\n---\n\nbody\n",
        );
        assert!(r2.engram.frontmatter.generated.is_none());
        assert_eq!(
            r2.engram.frontmatter.timestamp.unwrap().rfc3339(),
            "2020-01-01T00:00:00+00:00"
        );
        assert!(!r2.changes.iter().any(|c| c.starts_with("generated")));

        // An existing `generated` block is equally untouched.
        let r3 = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\ngenerated: { by: human:jordi, at: 2020-01-01T00:00:00+00:00 }\n---\n\nbody\n",
        );
        assert_eq!(
            r3.engram.frontmatter.generated.as_ref().unwrap().by,
            "human:jordi"
        );
        assert!(!r3.changes.iter().any(|c| c.starts_with("generated")));
    }

    #[test]
    fn comma_string_tags_are_normalized_to_a_list() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags: alpha, beta, gamma\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(
            r.engram.frontmatter.tags,
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn unknown_keys_and_body_survive_a_full_transform() {
        let source = "---\ntype: howto\ntitle: X\npermalink: legacy-kb/guides/x\ntags: a, b\nstatus_note: keep me\ncustom: 42\n---\n\nBody paragraph one.\n\nBody paragraph two.\n";
        let r = t(source);
        let out = emit_engram(&r.engram);
        assert!(out.contains("status_note: keep me"));
        assert!(out.contains("custom: 42"));
        assert!(out.ends_with("Body paragraph one.\n\nBody paragraph two.\n"));
    }

    #[test]
    fn a_fully_canonical_file_produces_zero_changes() {
        let source = "---\ntype: guide\ntitle: X\npermalink: guides/x\ntags:\n- t\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\nbody\n";
        let r = t(source);
        assert!(r.changes.is_empty(), "unexpected changes: {:?}", r.changes);
    }

    #[test]
    fn import_tree_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("legacy-kb");
        let domain = tmp.path().join("domain");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&domain).unwrap();
        std::fs::write(
            src.join("a.md"),
            "---\ntype: howto\ntitle: A\npermalink: legacy-kb/a\ntags:\n- t\n---\n\nbody text here\n",
        )
        .unwrap();

        let options = ImportOptions {
            src_dir: src.clone(),
            domain_dir: domain.clone(),
            type_map: default_type_map(),
            strip_prefix: None,
            dry_run: true,
            missing_status: MissingStatus::Stable,
        };
        let report = import_tree(&options).unwrap();
        assert_eq!(report.files_converted, 1);
        assert!(report.type_mapped >= 1);
        assert!(
            !domain.join("a.md").exists(),
            "dry run must not write files"
        );
    }

    #[test]
    fn import_tree_writes_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("legacy-kb");
        let domain = tmp.path().join("domain");
        std::fs::create_dir_all(src.join("guides")).unwrap();
        std::fs::create_dir_all(&domain).unwrap();
        std::fs::write(
            src.join("guides/a.md"),
            "---\ntype: howto\ntitle: A\npermalink: legacy-kb/guides/a\ntags: t, u\n---\n\nbody text here\n",
        )
        .unwrap();
        std::fs::write(src.join("notes.txt"), "asset\n").unwrap();

        let options = ImportOptions {
            src_dir: src.clone(),
            domain_dir: domain.clone(),
            type_map: default_type_map(),
            strip_prefix: None,
            dry_run: false,
            missing_status: MissingStatus::Stable,
        };
        let first = import_tree(&options).unwrap();
        assert_eq!(first.files_converted, 1);
        assert_eq!(first.files_copied, 1);
        assert!(domain.join("guides/a.md").exists());
        assert!(domain.join("notes.txt").exists());

        // Re-importing the already-converted output onto itself is a no-op:
        // the source and target now coincide, so this also proves writing
        // through emit_engram does not perpetually reformat a file.
        let reimport = ImportOptions {
            src_dir: domain.clone(),
            domain_dir: domain.clone(),
            ..options
        };
        let second = import_tree(&reimport).unwrap();
        assert_eq!(second.type_mapped, 0);
        assert_eq!(second.temporal_backfilled, 0);
        assert_eq!(second.sentinels_dropped, 0);
        assert_eq!(second.prefixes_stripped, 0);
        assert_eq!(second.collisions, 0);
        for f in &second.files {
            assert!(f.changes.is_empty(), "unexpected changes for {f:?}");
        }
    }

    #[test]
    fn colliding_permalinks_get_a_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("legacy-kb");
        let domain = tmp.path().join("domain");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&domain).unwrap();
        std::fs::write(
            src.join("first.md"),
            "---\ntype: engram\ntitle: First\npermalink: legacy-kb/shared\ntags:\n- t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody one text\n",
        )
        .unwrap();
        std::fs::write(
            src.join("second.md"),
            "---\ntype: engram\ntitle: Second\npermalink: legacy-kb/shared\ntags:\n- t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody two text\n",
        )
        .unwrap();

        let options = ImportOptions {
            src_dir: src.clone(),
            domain_dir: domain.clone(),
            type_map: default_type_map(),
            strip_prefix: None,
            dry_run: false,
            missing_status: MissingStatus::Stable,
        };
        let report = import_tree(&options).unwrap();
        assert_eq!(report.collisions, 1);
        let first_out = std::fs::read_to_string(domain.join("first.md")).unwrap();
        let second_out = std::fs::read_to_string(domain.join("second.md")).unwrap();
        assert!(first_out.contains("permalink: shared\n"));
        assert!(second_out.contains("permalink: shared-imported-1\n"));
    }

    #[test]
    fn map_override_extends_and_replaces_defaults() {
        let mut overrides = IndexMap::new();
        overrides.insert("spec".to_string(), "reference".to_string());
        overrides.insert("class".to_string(), "architecture".to_string());
        let merged = merge_type_map(&overrides);
        assert_eq!(merged.get("spec").map(String::as_str), Some("reference"));
        assert_eq!(
            merged.get("class").map(String::as_str),
            Some("architecture")
        );
        // Untouched defaults survive the merge.
        assert_eq!(merged.get("howto").map(String::as_str), Some("guide"));
    }

    fn copy_dir_all(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let target = to.join(e.file_name());
            if e.path().is_dir() {
                copy_dir_all(&e.path(), &target);
            } else {
                std::fs::copy(e.path(), target).unwrap();
            }
        }
    }

    fn linked_options(src: &Path, domain: &Path, dry_run: bool) -> ImportOptions {
        ImportOptions {
            src_dir: src.to_path_buf(),
            domain_dir: domain.to_path_buf(),
            type_map: default_type_map(),
            strip_prefix: None,
            dry_run,
            missing_status: MissingStatus::Stable,
        }
    }

    fn all_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    let rel = p
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, std::fs::read(&p).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// Distinct files the fixture's notes link that can be taken.
    const FIXTURE_LINKED: usize = 8;

    fn linked_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linked")
    }

    #[test]
    fn import_tree_takes_linked_files_and_rewrites_links() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let domain = tmp.path().join("domain");
        copy_dir_all(&linked_fixture(), &src);
        std::fs::create_dir_all(&domain).unwrap();

        let report = import_tree(&linked_options(&src, &domain, false)).unwrap();

        let bytes = std::fs::read(src.join("report.pdf.assets/page-3.png")).unwrap();
        assert_eq!(
            std::fs::read(domain.join("assets/report.pdf/page-3.png")).unwrap(),
            bytes
        );
        assert!(
            !domain.join("report.pdf.assets/page-3.png").exists(),
            "a taken file is not also copied to its old path"
        );
        let note = std::fs::read_to_string(domain.join("report.pdf.md")).unwrap();
        assert!(
            note.contains("](assets/report.pdf/page-3.png)"),
            "link not rewritten: {note}"
        );
        assert_eq!(report.files_linked, FIXTURE_LINKED);
    }

    #[test]
    fn files_no_note_links_are_still_copied_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let domain = tmp.path().join("domain");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&domain).unwrap();
        std::fs::write(
            src.join("a.md"),
            "---\ntype: engram\n---\n\n![p](pic.png)\n",
        )
        .unwrap();
        std::fs::write(src.join("pic.png"), b"png").unwrap();
        std::fs::write(src.join("extra.txt"), b"extra").unwrap();

        let report = import_tree(&linked_options(&src, &domain, false)).unwrap();

        assert_eq!(std::fs::read(domain.join("extra.txt")).unwrap(), b"extra");
        assert_eq!(report.not_linked, 1);
        assert_eq!(report.files_linked, 1);
        assert_eq!(report.files_copied, 1);
        assert!(!domain.join("pic.png").exists());
    }

    #[test]
    fn a_second_import_reuses_the_assets() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let domain = tmp.path().join("domain");
        copy_dir_all(&linked_fixture(), &src);
        std::fs::create_dir_all(&domain).unwrap();

        let first = import_tree(&linked_options(&src, &domain, false)).unwrap();
        let before = all_files(&domain);
        let stamp = |p: &str| {
            std::fs::metadata(domain.join(p))
                .unwrap()
                .modified()
                .unwrap()
        };
        let page = "assets/report.pdf/page-3.png";
        let modified = stamp(page);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = import_tree(&linked_options(&src, &domain, false)).unwrap();
        let after = all_files(&domain);

        assert_eq!(first.files_linked, second.files_linked);
        assert_eq!(second.files_linked, FIXTURE_LINKED);
        assert!(
            after.iter().all(|(rel, _)| !rel.contains("-2.")),
            "a duplicate was stored: {after:?}"
        );
        // No new files and no changed asset bytes (a note's `generated.at`
        // moves with the clock, so the notes are compared by name only).
        let names = |f: &[(String, Vec<u8>)]| f.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
        assert_eq!(names(&before), names(&after));
        let assets = |f: Vec<(String, Vec<u8>)>| {
            f.into_iter()
                .filter(|(r, _)| r.starts_with("assets/"))
                .collect::<Vec<_>>()
        };
        assert_eq!(assets(before), assets(after));
        assert_eq!(stamp(page), modified, "a reused asset was rewritten");
    }

    #[test]
    fn dry_run_writes_nothing_and_reports_the_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let domain = tmp.path().join("domain");
        copy_dir_all(&linked_fixture(), &src);
        std::fs::create_dir_all(&domain).unwrap();

        let report = import_tree(&linked_options(&src, &domain, true)).unwrap();

        assert!(all_files(&domain).is_empty(), "dry run wrote files");
        assert_eq!(report.files_linked, FIXTURE_LINKED);
        assert!(!report.not_taken.is_empty());
    }

    #[test]
    fn a_missing_mtime_falls_back_to_the_import_date() {
        let r = complete_engram(
            "---\ntype: engram\nstatus: current\ntags:\n- t\n---\n\nbody\n",
            &opts(&default_type_map(), MissingStatus::Stable, None),
        )
        .unwrap();
        assert_eq!(
            r.engram.frontmatter.recorded_at,
            Some(Day::from_ymd(2026, 7, 2).unwrap())
        );
        assert!(r.changes.iter().any(|c| c.contains("import date")));
    }

    #[test]
    fn a_name_that_slugifies_to_nothing_leaves_the_permalink_missing() {
        let map = default_type_map();
        let mut o = opts(&map, MissingStatus::Stable, None);
        o.rel_path = "日本語.md";
        let r = complete_engram("---\ntype: engram\ntags:\n- t\n---\n\nbody\n", &o).unwrap();
        assert_eq!(r.engram.frontmatter.permalink, None);
        assert!(!r.permalink_backfilled);
        assert!(r.changes.iter().any(|c| c.starts_with("warning:")));
    }

    #[test]
    fn a_file_that_needs_nothing_is_kept_as_it_is() {
        // No `generated`, which completion would add, and keys in an order
        // the emitter would not write: neither is a reason to rewrite it.
        let source = "---\ntitle: Alpha\ntype: engram\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n";
        let now = time::now().to_offset_utc();
        assert_eq!(complete_for_import(source, "alpha.md", now).unwrap(), None);
    }

    #[test]
    fn a_legacy_type_is_never_remapped_for_import() {
        let source = "---\ntype: note\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n";
        let now = time::now().to_offset_utc();
        assert_eq!(complete_for_import(source, "alpha.md", now).unwrap(), None);
        // A file that needs completing keeps its type too.
        let text = complete_for_import("---\ntype: howto\ntitle: H\n---\n\nbody\n", "h.md", now)
            .unwrap()
            .expect("the file needs completing");
        assert_eq!(
            parse_engram(&text).unwrap().frontmatter.engram_type,
            "howto"
        );
    }

    #[test]
    fn a_converted_file_is_completed_for_import() {
        let source = "---\ntype: document\ntitle: Report\n---\n\n# Report\n";
        let now = time::now().to_offset_utc();
        let text = complete_for_import(source, "report.pdf.md", now)
            .unwrap()
            .expect("the file needs completing");
        let engram = parse_engram(&text).unwrap();
        let fm = &engram.frontmatter;
        assert_eq!(fm.status.as_deref(), Some("stable"));
        assert_eq!(fm.tags, vec![IMPORTED_TAG.to_string()]);
        assert_eq!(fm.permalink.as_deref(), Some("report-pdf"));
        assert_eq!(fm.recorded_at, Some(now.day()));
        assert!(fm.generated.is_some());
    }

    #[test]
    fn a_failed_copy_leaves_no_note_that_links_the_missing_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let domain = tmp.path().join("domain");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&domain).unwrap();
        std::fs::write(
            src.join("a.md"),
            "---\ntype: engram\n---\n\n![p](pic.png)\n",
        )
        .unwrap();
        std::fs::write(src.join("pic.png"), b"png").unwrap();
        // A file where the `assets` folder must go.
        std::fs::write(domain.join("assets"), b"in the way").unwrap();

        let result = import_tree(&linked_options(&src, &domain, false));

        assert!(result.is_err());
        assert!(!domain.join("a.md").exists(), "the note was written");
    }
}
