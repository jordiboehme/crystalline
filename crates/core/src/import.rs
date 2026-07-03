//! A generic importer for markdown knowledge bases with YAML frontmatter.
//!
//! [`import_tree`] walks a source directory and converts every `.md` file
//! into canonical Engram shape: it ensures `type` is set (applying a legacy
//! type mapping table and tagging the original value), backfills missing
//! temporal metadata, drops sentinel open-ended dates, strips a leading
//! source-tree permalink prefix and adds a missing `timestamp`. Every other
//! frontmatter key and the body are preserved exactly: each file is read with
//! [`crate::parse::parse_engram_lossless`], the typed frontmatter is edited in
//! place and the result is written back with [`crate::emit::emit_engram`],
//! which keeps unknown keys in their original order and the body verbatim.
//! Non-markdown files are copied byte for byte.
//!
//! This is a pure file transformation: no database, network or config
//! registry lookup happens here, so the derived index never needs to exist
//! for an import to run. Idempotency is a property of the rules themselves,
//! not a marker: once a file carries an explicit `type`, temporal metadata, no
//! sentinel dates, an already-stripped permalink and a `timestamp`, running
//! the importer again on it is a no-op.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::emit::emit_engram;
use crate::engram::Engram;
use crate::parse::{ParseError, parse_engram, parse_engram_lossless};

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
}

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
    let now = Utc::now().fixed_offset();

    let mut report = ImportReport::default();

    for entry in &entries {
        if !entry.is_md {
            copy_verbatim(entry, options, &mut report)?;
            continue;
        }
        convert_markdown(
            entry,
            options,
            &strip_prefix,
            &options.type_map,
            now,
            &mut permalinks,
            &mut report,
        )?;
    }

    Ok(report)
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
            std::fs::copy(&entry.abs, &dest).map_err(io_err(&entry.abs))?;
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

#[allow(clippy::too_many_arguments)]
fn convert_markdown(
    entry: &Entry,
    options: &ImportOptions,
    strip_prefix: &str,
    type_map: &IndexMap<String, String>,
    now: DateTime<FixedOffset>,
    permalinks: &mut HashMap<String, String>,
    report: &mut ImportReport,
) -> Result<(), ImportError> {
    let source = match std::fs::read_to_string(&entry.abs) {
        Ok(s) => s,
        Err(e) => {
            report.files_skipped += 1;
            report
                .warnings
                .push(format!("{}: could not read file: {e}", entry.rel));
            report.files.push(FileChange {
                path: entry.rel.clone(),
                action: FileAction::Skipped,
                changes: Vec::new(),
            });
            return Ok(());
        }
    };

    let mtime = mtime_date(&entry.abs);
    let mut transformed = match transform_engram(&source, type_map, strip_prefix, mtime, now) {
        Ok(t) => t,
        Err(e) => {
            report.files_skipped += 1;
            report.warnings.push(format!("{}: {e}", entry.rel));
            report.files.push(FileChange {
                path: entry.rel.clone(),
                action: FileAction::Skipped,
                changes: Vec::new(),
            });
            return Ok(());
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

    let output = emit_engram(&transformed.engram);
    if !options.dry_run {
        let dest = options.domain_dir.join(&entry.rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
        std::fs::write(&dest, &output).map_err(io_err(&dest))?;
    }

    report.files_converted += 1;
    if transformed.type_mapped {
        report.type_mapped += 1;
    }
    report.temporal_backfilled += transformed.temporal_backfilled;
    report.sentinels_dropped += transformed.sentinels_dropped;
    if transformed.prefix_stripped {
        report.prefixes_stripped += 1;
    }
    report.files.push(FileChange {
        path: entry.rel.clone(),
        action: FileAction::Converted,
        changes: transformed.changes,
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

/// The result of transforming one Engram's frontmatter. `engram` is ready to
/// pass to [`emit_engram`] once any permalink collision is resolved by the
/// caller.
struct Transformed {
    engram: Engram,
    changes: Vec<String>,
    type_mapped: bool,
    temporal_backfilled: usize,
    sentinels_dropped: usize,
    prefix_stripped: bool,
}

/// Apply every per-file transformation rule to one Engram's source text.
/// Pure function, no filesystem access, so it is exercised directly in unit
/// tests without a source tree on disk.
fn transform_engram(
    source: &str,
    type_map: &IndexMap<String, String>,
    strip_prefix: &str,
    mtime: Option<NaiveDate>,
    now: DateTime<FixedOffset>,
) -> Result<Transformed, ParseError> {
    let lossless = parse_engram_lossless(source)?;
    let mut engram = lossless.engram;
    let mut changes = Vec::new();
    let mut type_mapped = false;
    let mut temporal_backfilled = 0usize;
    let mut sentinels_dropped = 0usize;
    let mut prefix_stripped = false;

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
        fm.status = Some("legacy".to_string());
        fm.temporal_confidence = Some("inferred".to_string());
        temporal_backfilled += 1;
        changes.push("status: missing -> `legacy` (temporal_confidence: inferred)".to_string());
    }
    if fm.recorded_at.is_none()
        && let Some(date) = mtime
    {
        fm.recorded_at = Some(date);
        temporal_backfilled += 1;
        changes.push(format!("recorded_at: missing -> `{date}` (file mtime)"));
    }

    // c. Drop sentinel far-future `valid_to` and far-past `valid_from`
    // dates: absence is what expresses open-ended validity.
    if let Some(to) = fm.valid_to
        && to.year() >= SENTINEL_FUTURE_YEAR
    {
        fm.valid_to = None;
        sentinels_dropped += 1;
        changes.push(format!("valid_to: dropped sentinel `{to}`"));
    }
    if let Some(from) = fm.valid_from
        && from.year() <= SENTINEL_PAST_YEAR
    {
        fm.valid_from = None;
        sentinels_dropped += 1;
        changes.push(format!("valid_from: dropped sentinel `{from}`"));
    }

    // d. Strip a leading source-prefix permalink segment. A missing
    // permalink stays missing; sync derives one later.
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

    // e. Add `timestamp` only if absent.
    if fm.timestamp.is_none() {
        fm.timestamp = Some(now);
        changes.push(format!("timestamp: missing -> `{}`", now.to_rfc3339()));
    }

    // f. Tags: `parse_engram_lossless` already normalizes a comma-separated
    // string into a list, so nothing further is needed here.

    Ok(Transformed {
        engram,
        changes,
        type_mapped,
        temporal_backfilled,
        sentinels_dropped,
        prefix_stripped,
    })
}

fn mtime_date(path: &Path) -> Option<NaiveDate> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dt: DateTime<Utc> = modified.into();
    Some(dt.date_naive())
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

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-07-02T00:00:00+00:00").unwrap()
    }

    fn t(source: &str) -> Transformed {
        transform_engram(source, &default_type_map(), "legacy-kb", None, now()).unwrap()
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
    fn missing_status_backfills_legacy_and_inferred() {
        let r =
            t("---\ntype: engram\ntitle: X\ntags:\n  - t\nrecorded_at: 2026-01-01\n---\n\nbody\n");
        assert_eq!(r.engram.frontmatter.status.as_deref(), Some("legacy"));
        assert_eq!(
            r.engram.frontmatter.temporal_confidence.as_deref(),
            Some("inferred")
        );
        assert_eq!(r.temporal_backfilled, 1);
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
        let date = NaiveDate::from_ymd_opt(2026, 3, 4).unwrap();
        let r = transform_engram(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\n---\n\nbody\n",
            &default_type_map(),
            "legacy-kb",
            Some(date),
            now(),
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
    fn missing_permalink_stays_missing() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert_eq!(r.engram.frontmatter.permalink, None);
    }

    #[test]
    fn timestamp_is_added_only_when_absent() {
        let r = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nbody\n",
        );
        assert!(r.engram.frontmatter.timestamp.is_some());

        let r2 = t(
            "---\ntype: engram\ntitle: X\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2020-01-01T00:00:00+00:00\n---\n\nbody\n",
        );
        assert_eq!(
            r2.engram.frontmatter.timestamp.unwrap().to_rfc3339(),
            "2020-01-01T00:00:00+00:00"
        );
        assert!(!r2.changes.iter().any(|c| c.starts_with("timestamp")));
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
}
