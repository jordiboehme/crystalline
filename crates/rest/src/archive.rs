//! Domain archives (admin): the zip download of a whole domain, and the
//! two-step upload that puts one back.
//!
//! Two decisions the download encodes. The archive is built in memory as one
//! `Vec<u8>` under `spawn_blocking` rather than streamed - a zip writer is
//! synchronous, and at the sizes this surface serves a buffer beats a
//! streaming writer's complexity. And the download is served on a read-only
//! instance: it mutates nothing, and a read-only mirror is exactly where an
//! operator wants a backup they can take.
//!
//! The upload is the untrusted direction, and it is deliberately two calls
//! over one engine verb: `preview` runs [`crate::engine::Engine::import_domain_files`]
//! with `dry_run`, `import` runs the same verb for real, so the report a user
//! approves and the work that follows can never disagree about an entry. Both
//! screen the archive first, and the screen answers two different ways on
//! purpose:
//!
//! - A *hygiene* failure refuses the WHOLE request with a 422 naming the
//!   reason: not a zip, too many entries, an entry that decompresses past the
//!   per-entry cap, a total past the archive cap, an entry name that is not
//!   UTF-8, any path that could escape the domain root. A hostile archive
//!   gets no partial import.
//! - A *per-entry* demotion - a non-`.md` file no note links, a MANIFEST, an
//!   OKF reserved name, bytes that are not UTF-8 text, a hard verify error,
//!   and anything under `reflections/` when the archive's own MANIFEST keeps
//!   a Reflection there - stays in the report as that entry's status and
//!   never reaches the engine.
//!
//! Attachments travel in both directions. The download carries every
//! attachment the domain holds under its `assets/` path, whichever storage
//! kind the domain has, so an export of a file domain restores into a virtual
//! one and back. The upload takes an `assets/` entry when it passes the same
//! gates an upload through the file route passes - the path rules, the
//! extension allowlist and the size ceiling - and stores it through the
//! engine's attachment seam rather than through the engram import verb, which
//! is about permalinks and parses a PNG does not have. Each of those gates
//! answers per entry, with its own reason, and each is the reserved folder's
//! rule rather than markdown's: an entry under `assets/` is never judged by
//! the "only .md entries are imported" rule, and an `assets/x.md` is refused
//! outright instead of landing as an engram inside the reserved folder where
//! no walk would find it again.
//!
//! One consequence worth naming rather than discovering: the seam marks the
//! domain as owing a consolidation sweep, so an import that carries
//! attachments does mark it pending where an engram-only restore does not.
//! That is the right way round - the markdown of a restore is knowledge the
//! agent already had, while a restored screenshot is a file nobody has read
//! yet - and it comes from calling the seam rather than from a rule this
//! layer keeps of its own.
//!
//! What this layer screens, and what it leaves to the engine: the screen here
//! covers zip-slip, the size and count caps, `assets/` entries, non-`.md`
//! entries, MANIFEST at any depth and the OKF reserved names (`index.md`,
//! `log.md`). The engine's `import_domain_files` sees markdown only - the
//! attachments are settled here, against the seam - and it screens the same
//! MANIFEST, non-markdown and reserved
//! names again, plus its own containment check - that duplication is
//! deliberate defense in depth, and this module does not silently rely on it:
//! the outer screen exists so a preview's precedence (`ignored` before
//! `invalid`) is decided before any verify rule runs, and the inner one exists
//! so an engine caller that is not this surface is safe too.
//!
//! Every name comparison in that screen is case-INSENSITIVE, at both layers,
//! and the reason is the filesystem rather than the format. A file domain
//! lives on APFS or NTFS on most installs, where an entry called `manifest.md`
//! or `Log.md` addresses the existing `MANIFEST.md` or `log.md`: the engine's
//! existence probe resolves to the real file and, under `policy=overwrite`,
//! the write renames onto it, replacing its bytes while the on-disk name never
//! changes. For a MANIFEST and for a log that is permanent damage - neither is
//! ever regenerated - so an exact-string screen would hand an uploaded archive
//! the one thing this endpoint must not allow. `index.md` is usually rebuilt
//! within the same request, but not when an operator has turned `index.files`
//! off, and a rule that is only safe under one setting is not a rule.
//!
//! This is deliberately stricter than `crystalline_core::is_reserved_file`,
//! whose exact, case-sensitive match stays as it is: that rule is about what
//! Crystalline generates and exports, where one deterministic answer on every
//! platform is worth more than filesystem realism. Import is the one direction
//! that faces a filesystem holding files it did not write, so it screens on
//! what the filesystem will do rather than on what the format says. Being
//! stricter here costs nothing in agreement: `preview` and `import` share one
//! `read_archive`, so both apply the identical rule and a preview can never
//! promise something the import then contradicts.
//!
//! Locks: preview and import deliberately take neither the domain-admin mutex
//! nor the join fence, the same ruling the sync POST carries - the domain is
//! resolved per call, and an unregister racing an import lands in the same
//! benign engine-level window that the MCP surface and the daemon's poller
//! already share (worst case, stale index rows on the virtual arm), so a
//! REST-local mutex could not close it and would only queue uploads behind
//! minutes-long admin work. Fixing it for real means fixing it in the engine.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, body::Bytes};

use crate::domain_view::DomainView;
use crate::engine::REFLECTION_NOT_IMPORTED;

use super::auth::Identity;
use super::engrams::ValidateFinding;
use super::{
    ApiError, ApiPath, ApiQuery, ProblemDetail, RestState, refuse_read_only, require_domain_read,
};

/// How many entries an uploaded archive may hold.
const MAX_ARCHIVE_ENTRIES: usize = 1000;

/// The largest a single markdown entry may be once decompressed, in bytes.
const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

/// The largest an `assets/` entry may be once decompressed: the attachment
/// ceiling itself, since what this endpoint would do with such an entry is
/// store it through the attachment seam, which enforces exactly this bound.
/// Markdown keeps the tighter cap above - an engram that size is a mistake,
/// a 4 MiB screenshot is not.
const MAX_ASSET_ENTRY_BYTES: u64 = crystalline_core::MAX_ATTACHMENT_BYTES;

// Why an `assets/` entry was refused, in the words every import route
// reports: named constants in core, because the same strings are asserted by
// the tests and read by a user in the preview dialog.
use crystalline_core::attachment::{
    ASSET_EXTENSION_REASON, ASSET_MARKDOWN_REASON, ASSET_PATH_REASON, ASSET_SIZE_REASON,
};
/// Why a linked file is not written: the domain holds the same bytes at the
/// attachment path it would take.
const SAME_FILE_REASON: &str = "the same file is already there";
/// Why a linked file of the archive is not stored on its own: another file
/// of the archive has the same bytes, and the attachment path that follows
/// holds them.
const SAME_BYTES_REASON: &str = "the same bytes are stored at";

/// The largest a whole archive may be once decompressed, in bytes.
///
/// Roughly three times the compressed body these routes accept
/// ([`super::ARCHIVE_BODY_BYTES`], 64 MiB), the same ratio this cap carried
/// when the body limit was the general one: enough headroom that a domain of
/// ordinary markdown, which deflates far better than three to one, is bounded
/// by the transport rather than by this, and low enough that the memory a
/// single import can make this process hold stays a number an operator can
/// predict. Every accepted entry's decompressed bytes are buffered before the
/// first write, so this is the real memory ceiling of an import, and the
/// screening that meters against it runs on the blocking pool rather than on a
/// runtime worker.
pub(crate) const MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024;

/// `GET /domains/{domain}/archive` - the whole domain as a zip.
///
/// Portable across storage kinds: a file domain's markdown comes off disk and
/// a virtual domain's out of the database, MANIFEST included either way, so
/// what lands in the browser is a folder that can be registered anywhere.
///
/// A pure read, and deliberately still served when the instance is read-only:
/// the archive download IS the backup story of a read-only mirror, so
/// refusing it there would take the feature away exactly where it is wanted.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/archive",
    tag = "domains",
    operation_id = "download_domain_archive",
    summary = "Download a whole domain as a zip.",
    description = "Admin only. Every file of the domain - MANIFEST and the \
                   `assets/` attachments included - as one zip, read from \
                   whichever source of truth the domain has: markdown on disk \
                   for a file domain, the database for a virtual one. A pure \
                   read, so it is served even on a read-only instance, which \
                   is exactly where an operator wants a backup to take.",
    params(("domain" = String, Path, description = "The registered domain to archive.")),
    responses(
        (
            status = 200,
            description = "The archive, as an attachment named after the \
                           domain.",
            // The body is raw zip bytes. Declared as a string because utoipa's
            // schema for `Vec<u8>` is an array of integers, which would
            // describe this response as JSON; no typed client consumes this
            // route, so the content type carries the meaning.
            body = String,
            content_type = "application/zip",
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, or the trusted-header \
                           identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn download(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
) -> Result<Response, ApiError> {
    identity.require_admin()?;
    // No refuse_read_only: this is a read. See the doc comment.
    //
    // Scoped like every other domain-addressed read, even though an admin is
    // the only caller who reaches it and an admin may see every domain: an
    // export is the whole domain in one file, so if the role gate above ever
    // widens, the export of a domain the caller may not see must already be
    // the 404 an unregistered name gets.
    require_domain_read(&state, &identity, &domain).await?;
    let files = state.engine.domain_files(&domain).await?;
    let bytes = tokio::task::spawn_blocking(move || build_zip(&files))
        .await
        .map_err(|e| ApiError::internal(format!("archive build task failed: {e}")))?
        .map_err(|e| ApiError::internal(format!("building the archive: {e}")))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", archive_filename(&domain)),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// The suggested filename, defanged to filename-safe ASCII: nothing a domain
/// name could carry can put a separator, a quote or a header break into the
/// `Content-Disposition` this builds.
fn archive_filename(domain: &str) -> String {
    let safe: String = domain
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{safe}-archive.zip")
}

/// Build the whole zip in memory.
///
/// Synchronous, so callers run it on the blocking pool. The buffer is
/// unbounded: responses are not subject to the surface's request-body limit,
/// which is fine at the sizes a domain reaches here but would balloon the
/// daemon on a pathological multi-hundred-MiB domain. If one ever appears,
/// the shape to switch to is a temp file plus a `ReaderStream` body.
fn build_zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, zip::result::ZipError> {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        for (path, content) in files {
            writer.start_file(path.as_str(), options)?;
            writer.write_all(content)?;
        }
        writer.finish()?;
    }
    Ok(cursor.into_inner())
}

/// What became of one entry of an uploaded archive.
///
/// Every entry the archive carried gets a line, screened ones included: a
/// preview that quietly dropped what it would not import would be a worse
/// answer than one that names it.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One entry of an uploaded archive and what became of \
                        it. A preview reports `new`, `collides`, `invalid` or \
                        `ignored`; an import reports `created`, \
                        `overwritten`, `skipped`, `invalid` or `ignored`.")]
pub struct ArchiveEntryReport {
    /// The entry's path inside the archive, domain-relative.
    #[schema(example = "alpha.md")]
    pub path: String,
    /// preview: new | collides | invalid | ignored.
    /// import: created | overwritten | skipped | invalid | ignored.
    #[schema(example = "new")]
    pub status: String,
    /// The permalink the entry claims, once it parsed far enough to have one.
    #[schema(example = "alpha")]
    pub permalink: Option<String>,
    /// Why the entry was not written, in the words of whatever refused it.
    pub reason: Option<String>,
    /// How many bytes the entry holds, for an attachment. `null` for a
    /// markdown entry, whose size a preview has no use for, and for an entry
    /// that was refused before it was ever decompressed.
    #[schema(example = 20480)]
    pub bytes: Option<u64>,
    /// The verify findings over this entry's markdown, the same families
    /// `POST /validate` runs. Empty for an entry that was never read.
    pub findings: Vec<ValidateFinding>,
}

/// What a preview or an import answers: a line per entry, plus the tallies a
/// confirmation dialog counts with.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "The per-entry report of a preview or an import, with \
                        the counters a confirmation dialog summarizes. The \
                        counters are tallied from the entries above them, so \
                        the two can never disagree.")]
pub struct ArchiveReport {
    /// The domain the archive was aimed at.
    #[schema(example = "eng")]
    pub domain: String,
    /// True for a preview, false for an import that really wrote.
    pub dry_run: bool,
    /// Every entry of the archive, in the order the archive holds them.
    pub entries: Vec<ArchiveEntryReport>,
    /// Preview only: entries that would be created.
    pub new: usize,
    /// Preview only: entries whose path or permalink is already taken.
    pub collides: usize,
    /// Import only: entries written, created and overwritten together.
    pub written: usize,
    /// Import only: entries an existing path or permalink held back.
    pub skipped: usize,
    /// Entries refused as not importable: unparseable, not UTF-8 text,
    /// carrying a hard verify error, or an attachment whose path or size the
    /// rules refuse. Never written under either policy.
    pub invalid: usize,
    /// Entries an archive may carry but a domain never imports: a MANIFEST, a
    /// generated OKF index or log, anything that is neither markdown nor an
    /// attachment of an allowed type that a note links.
    pub ignored: usize,
    /// How many distinct files the notes link to were taken in as
    /// attachments. Each is listed in `entries` at its `assets/` path.
    pub files_linked: usize,
    /// Every link to a local file that was not taken, once per note and
    /// target. The link is left as it was written.
    pub not_taken: Vec<ArchiveNotTaken>,
}

/// One link a note makes to a file the import did not take in.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "A link from an archived note to a local file that \
                        was not taken in as an attachment, and why.")]
pub struct ArchiveNotTaken {
    /// The path of the note that links the file.
    #[schema(example = "report.pdf.md")]
    pub note: String,
    /// The link target as the note names it, decoded, without a fragment.
    #[schema(example = "missing.png")]
    pub target: String,
    /// Why the file was not taken.
    pub reason: ArchiveNotTakenReason,
}

/// Why a linked file was not taken: `missing` (no such entry in the
/// archive), `outside` (the link leaves the archive), `not_allowed` (not an
/// attachment type), `too_large` (past the attachment size limit) or
/// `bad_name` (no usable attachment name could be made from it).
#[derive(Debug, Clone, Copy, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveNotTakenReason {
    /// No such entry in the archive.
    Missing,
    /// The link leaves the archive.
    Outside,
    /// Not an attachment type.
    NotAllowed,
    /// Past the attachment size limit.
    TooLarge,
    /// No usable attachment name could be made from it.
    BadName,
}

impl From<crystalline_core::linked::NotTaken> for ArchiveNotTaken {
    fn from(item: crystalline_core::linked::NotTaken) -> Self {
        use crystalline_core::linked::NotTakenReason as Reason;
        Self {
            note: item.note,
            target: item.target,
            reason: match item.reason {
                Reason::Missing => ArchiveNotTakenReason::Missing,
                Reason::Outside => ArchiveNotTakenReason::Outside,
                Reason::NotAllowed => ArchiveNotTakenReason::NotAllowed,
                Reason::TooLarge => ArchiveNotTakenReason::TooLarge,
                Reason::BadName => ArchiveNotTakenReason::BadName,
            },
        }
    }
}

/// The archive's entries that are neither markdown nor under `assets/`, kept
/// for the notes that link them.
///
/// An entry that could become an attachment (an allowed type within the
/// attachment ceiling) is held with its bytes; any other is held by its size
/// alone, so the linked-file planner can say it is not allowed or too large
/// without its bytes being decompressed or kept.
#[derive(Debug, Default)]
struct LooseFiles {
    /// The entries read, by path.
    read: crystalline_core::linked::MemSource,
    /// The entries left unread, by path, with their size.
    unread: std::collections::BTreeMap<String, u64>,
}

impl LooseFiles {
    fn contains(&self, path: &str) -> bool {
        self.read.files.contains_key(path) || self.unread.contains_key(path)
    }

    fn is_empty(&self) -> bool {
        self.read.files.is_empty() && self.unread.is_empty()
    }
}

impl crystalline_core::linked::LinkedSource for LooseFiles {
    fn lookup(&self, rel: &str) -> crystalline_core::linked::Lookup {
        use crystalline_core::linked::Lookup;
        match self.read.lookup(rel) {
            Lookup::Missing => self
                .unread
                .get(rel)
                .map_or(Lookup::Missing, |&len| Lookup::File { len }),
            found => found,
        }
    }

    fn sha256(&self, rel: &str) -> std::io::Result<String> {
        self.read.sha256(rel)
    }
}

/// The query `POST .../archive/import` takes.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportQuery {
    /// How a path that already exists is treated: `skip` (the default) leaves
    /// it alone, `overwrite` replaces it. Anything else is refused.
    #[serde(default)]
    #[param(example = "skip")]
    pub policy: Option<String>,
}

/// Whether an uploaded entry carries an OKF reserved name at any depth,
/// compared without regard to case.
///
/// Not `crystalline_core::is_reserved_path`, which matches exactly: see the
/// module docs for why the import direction is deliberately the stricter of
/// the two. The engine's `import_domain_files` applies the same widened rule
/// as the inner defense.
fn is_reserved_upload(path: &str) -> bool {
    std::path::Path::new(path).file_name().is_some_and(|name| {
        name.eq_ignore_ascii_case(crystalline_core::INDEX_FILE)
            || name.eq_ignore_ascii_case(crystalline_core::LOG_FILE)
    })
}

/// One entry after the screen: ready to import, or already decided.
enum Screened {
    /// Markdown this endpoint will hand to the engine.
    Entry { path: String, content: String },
    /// An attachment this endpoint will hand to the attachment seam.
    Asset { path: String, bytes: Vec<u8> },
    /// An entry a domain never imports, whatever it holds.
    Ignored { path: String, reason: String },
    /// An entry that cannot be imported as it stands.
    Invalid { path: String, reason: String },
}

/// What an `assets/` entry's NAME alone decides, before a byte is read.
///
/// `None` means the name passes every upload gate and the bytes are worth
/// decompressing. The two demotion classes are deliberately different: an
/// extension that is not on the allowlist (`.exe`, and `.md`, which is an
/// Engram rather than an attachment) is a KIND of file a domain never imports,
/// which is what `ignored` means here and is the same verdict a `.txt` at the
/// root gets; a name that breaks a path rule is this one entry failing, which
/// is `invalid`.
///
/// The markdown case is called out separately because it is the confusing one:
/// `assets/notes.md` ends in `.md`, so the markdown rule below would have
/// imported it as an engram whose path lives inside the reserved folder - an
/// engram no walk would ever find again.
fn asset_name_verdict(path: &str) -> Option<Screened> {
    let reason = match crystalline_core::validate_asset_path(path) {
        Ok(()) => return None,
        Err(crystalline_core::AssetPathError::DisallowedExtension) => {
            if path.to_lowercase().ends_with(".md") {
                ASSET_MARKDOWN_REASON
            } else {
                ASSET_EXTENSION_REASON
            }
        }
        Err(_) => ASSET_PATH_REASON,
    };
    let path = path.to_string();
    Some(if reason == ASSET_PATH_REASON {
        Screened::Invalid {
            path,
            reason: reason.to_string(),
        }
    } else {
        Screened::Ignored {
            path,
            reason: reason.to_string(),
        }
    })
}

/// Why an entry is not imported when its name is one this OS cannot hold as
/// a file. Only Windows has such names today (zip 9.1 and later says so):
/// a component over 255 UTF-16 units, a device name like `CON`, a trailing
/// dot or space, or one of `<>"|?*`.
const NOT_A_FILE_NAME_HERE_REASON: &str = "the name is not a valid file name on this system";

/// Whether an archive entry's name leaves the extraction root: a leading
/// `/` (which covers `//server`), an empty, `.` or `..` component, a `:`
/// anywhere (a drive letter or an alternate data stream), a `\` anywhere
/// (a Windows separator, a UNC prefix) or a NUL. The same rules as the
/// remote crate's `to_platform_path`, judged on the name alone, so the
/// answer is the same on every OS.
fn escapes_the_root(raw: &str) -> bool {
    raw.starts_with('/')
        || raw.contains('\0')
        || raw
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".." || c.contains(':') || c.contains('\\'))
}

/// What an archive entry's name alone decides, before anything reads it.
///
/// An escape refuses the whole archive. `enclosed` is zip's own
/// `enclosed_name` check, kept as a second line behind
/// [`escapes_the_root`]: since zip 9.1 it also refuses, on Windows only, a
/// name that is not a valid Windows file name, which is not an escape. Such
/// a name is screened per entry instead: an `assets/` entry by the
/// attachment gate when that gate has a reason of its own (a component that
/// long is always over the attachment path limit), any other with
/// [`NOT_A_FILE_NAME_HERE_REASON`]. So a too-long attachment name gets the
/// same answer on every OS.
fn entry_name_verdict(raw: &str, enclosed: bool) -> Result<Option<Screened>, ApiError> {
    if escapes_the_root(raw) {
        return Err(ApiError::unprocessable(format!(
            "archive entry '{raw}' escapes the extraction root; refusing the archive"
        )));
    }
    if enclosed {
        return Ok(None);
    }
    if crystalline_core::is_under_assets(raw)
        && let Some(decided) = asset_name_verdict(raw)
    {
        return Ok(Some(decided));
    }
    Ok(Some(Screened::Invalid {
        path: raw.to_string(),
        reason: NOT_A_FILE_NAME_HERE_REASON.to_string(),
    }))
}

/// Whether the archive's root `MANIFEST.md` declares `reflection: on`.
/// Read under the per-entry ceiling the screen applies to markdown; a
/// missing, oversized or unparseable MANIFEST declares nothing.
fn archive_keeps_a_reflection(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
) -> Result<bool, ApiError> {
    use std::io::Read;
    let entry = match archive.by_name("MANIFEST.md") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(false),
        Err(e) => {
            return Err(ApiError::unprocessable(format!(
                "unreadable archive entry 'MANIFEST.md': {e}"
            )));
        }
    };
    if entry.size() > MAX_ENTRY_BYTES {
        return Ok(false);
    }
    let mut buf = Vec::new();
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            ApiError::unprocessable(format!("unreadable archive entry 'MANIFEST.md': {e}"))
        })?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Ok(false);
    }
    let Ok(source) = String::from_utf8(buf) else {
        return Ok(false);
    };
    Ok(crystalline_core::parse_engram(&source)
        .map(|engram| crystalline_core::Manifest::from_engram(&engram, &source).reflection())
        .unwrap_or_default()
        == crystalline_core::ReflectionSwitch::On)
}

/// Open and screen an uploaded archive.
///
/// Hygiene refusals reject the WHOLE request, because a hostile archive gets
/// no partial import: not a zip, too many entries, a markdown entry oversized
/// by declaration OR by actual decompressed bytes, an archive past the
/// decompressed total, a name that is not UTF-8, and any traversal shape -
/// including one under `assets/`, which the containment screen decides before
/// the attachment rules are ever consulted.
///
/// Per-entry demotions stay in the report: a non-`.md` entry outside
/// `assets/`, a MANIFEST, a reserved name, markdown that is not UTF-8, and -
/// under `assets/` - a path the attachment rules refuse, an extension that is
/// not on the allowlist, markdown where an attachment belongs, and an
/// attachment past the attachment ceiling. That last one is the one asymmetry
/// worth remembering: an oversized markdown entry refuses the archive, an
/// oversized attachment refuses only itself.
///
/// Where the archive's own root `MANIFEST.md` declares `reflection: on`,
/// every entry its Reflection keeps (under `reflections/`) is ignored with
/// [`REFLECTION_NOT_IMPORTED`]: the MANIFEST is dropped before the engine
/// sees the batch, so this screen applies it (R33), and the engine applies
/// the receiving domain's own switch.
///
/// A non-`.md` entry outside `assets/` is also kept in the returned
/// [`LooseFiles`], for the notes that may link it: such an entry is only
/// reported as ignored when no note takes it in. It is read under the
/// attachment ceiling and counted toward the total like an `assets/` entry,
/// and one whose type or size rules it out is known by its size alone.
///
/// Size limits are enforced on the bytes actually decompressed, never on the
/// header's claim alone. Synchronous and metering up to
/// [`MAX_TOTAL_BYTES`] of inflation, so callers run it on the blocking pool.
fn read_archive(bytes: &[u8]) -> Result<(Vec<Screened>, LooseFiles), ApiError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ApiError::unprocessable(format!("not a readable zip archive: {e}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ApiError::unprocessable(format!(
            "the archive has {} entries; the limit is {MAX_ARCHIVE_ENTRIES}",
            archive.len()
        )));
    }
    // The archive's MANIFEST first, by name, so the one pass below knows
    // whether a Reflection keeps `reflections/`.
    let reflection = archive_keeps_a_reflection(&mut archive)?;
    let layout = crystalline_core::DomainLayout::bare(crystalline_core::ReflectionMode::Effective);
    let mut total: u64 = 0;
    let mut out = Vec::new();
    let mut loose = LooseFiles::default();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| ApiError::unprocessable(format!("unreadable archive entry: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        // The decoded name is only as trustworthy as its bytes: zip falls back
        // to CP437 for an entry whose UTF-8 flag is unset, which turns
        // arbitrary bytes into a name that looks clean and no longer matches
        // what the archive really carried. Rather than import under a name
        // nobody wrote, the whole archive is refused - every archive this
        // surface produces is UTF-8.
        if std::str::from_utf8(entry.name_raw()).is_err() {
            return Err(ApiError::unprocessable(
                "an archive entry name is not UTF-8; refusing the archive",
            ));
        }
        let raw = entry
            .name()
            .map_err(|_| {
                ApiError::unprocessable(
                    "an archive entry name cannot be read; refusing the archive",
                )
            })?
            .into_owned();
        if let Some(decided) = entry_name_verdict(&raw, entry.enclosed_name().is_some())? {
            out.push(decided);
            continue;
        }
        // A Reflection's pages and files are never engrams or attachments:
        // answered before both screens below, so a `reflections/assets/`
        // file is named for what it is rather than as "not markdown".
        if reflection
            && matches!(
                layout.classify(&raw),
                crystalline_core::PathClass::Reflection(_)
            )
        {
            out.push(Screened::Ignored {
                path: raw,
                reason: REFLECTION_NOT_IMPORTED.to_string(),
            });
            continue;
        }
        // The reserved folder is answered first, and before the markdown rule
        // below, because both directions of that order would be wrong: an
        // `assets/` entry that is not markdown must not be dismissed as "only
        // .md entries are imported", and an `assets/x.md` must not be taken
        // for an engram. The folder is matched case-insensitively, the way the
        // reservation itself is, so `Assets/x.png` lands here and is then
        // refused by the path rules rather than imported as knowledge.
        if crystalline_core::is_under_assets(&raw) {
            if let Some(decided) = asset_name_verdict(&raw) {
                out.push(decided);
                continue;
            }
            // Same two-step as the markdown arm below - the declared size is a
            // claim, the metered bytes are the truth - against the attachment
            // ceiling rather than the markdown one. Both answers are per-entry:
            // an attachment somebody exported at 12 MiB is a file this domain
            // will not take, not an archive worth refusing whole.
            if entry.size() > MAX_ASSET_ENTRY_BYTES {
                out.push(Screened::Invalid {
                    path: raw,
                    reason: ASSET_SIZE_REASON.to_string(),
                });
                continue;
            }
            let mut buf = Vec::new();
            {
                use std::io::Read;
                let mut limited = (&mut entry).take(MAX_ASSET_ENTRY_BYTES + 1);
                limited.read_to_end(&mut buf).map_err(|e| {
                    ApiError::unprocessable(format!("unreadable archive entry '{raw}': {e}"))
                })?;
            }
            // Metered bytes count toward the archive total whatever becomes of
            // the entry, so a pile of oversized attachments is still bounded
            // work rather than an unbounded one.
            total += buf.len() as u64;
            if total > MAX_TOTAL_BYTES {
                return Err(ApiError::unprocessable(format!(
                    "the archive unpacks past the {MAX_TOTAL_BYTES}-byte total limit"
                )));
            }
            if buf.len() as u64 > MAX_ASSET_ENTRY_BYTES {
                out.push(Screened::Invalid {
                    path: raw,
                    reason: ASSET_SIZE_REASON.to_string(),
                });
                continue;
            }
            out.push(Screened::Asset {
                path: raw,
                bytes: buf,
            });
            continue;
        }
        // Anything else that is not markdown is not imported as it is, but a
        // note may link it, so it is kept for the planner: read when it could
        // become an attachment, metered like an `assets/` entry, and known by
        // its size alone otherwise. It stays reported as ignored unless a
        // note takes it in.
        if !raw.to_lowercase().ends_with(".md") {
            let name = raw.rsplit('/').next().unwrap_or(&raw);
            if crystalline_core::attachment_mime(name).is_none()
                || entry.size() > MAX_ASSET_ENTRY_BYTES
            {
                loose.unread.insert(raw.clone(), entry.size());
            } else {
                let mut buf = Vec::new();
                {
                    use std::io::Read;
                    let mut limited = (&mut entry).take(MAX_ASSET_ENTRY_BYTES + 1);
                    limited.read_to_end(&mut buf).map_err(|e| {
                        ApiError::unprocessable(format!("unreadable archive entry '{raw}': {e}"))
                    })?;
                }
                total += buf.len() as u64;
                if total > MAX_TOTAL_BYTES {
                    return Err(ApiError::unprocessable(format!(
                        "the archive unpacks past the {MAX_TOTAL_BYTES}-byte total limit"
                    )));
                }
                if buf.len() as u64 > MAX_ASSET_ENTRY_BYTES {
                    loose.unread.insert(raw.clone(), buf.len() as u64);
                } else {
                    loose.read.files.insert(raw.clone(), buf);
                }
            }
            out.push(Screened::Ignored {
                path: raw,
                reason: "only .md entries are imported".to_string(),
            });
            continue;
        }
        // Case-insensitive on purpose: see the module docs. A `manifest.md`
        // would land on the real `MANIFEST.md` on any case-insensitive
        // filesystem, which is most of them.
        if std::path::Path::new(&raw)
            .file_name()
            .is_some_and(|name| crate::engine::names_the_manifest(&name.to_string_lossy()))
        {
            out.push(Screened::Ignored {
                path: raw,
                reason: "the archive MANIFEST is ignored; the domain keeps its own".to_string(),
            });
            continue;
        }
        // The OKF reserved names, and case-insensitively for the same reason
        // the MANIFEST above is: on APFS or NTFS a `Log.md` entry renames onto
        // the existing `log.md`, and nothing in Crystalline ever regenerates a
        // log. See the module docs for why this is stricter than
        // `crystalline_core::is_reserved_file` on purpose.
        if is_reserved_upload(&raw) {
            out.push(Screened::Ignored {
                path: raw,
                // Worded differently from the engine's line for the same case,
                // so a report says which of the two screens answered - and so a
                // test can tell a regression here from the inner screen quietly
                // covering for it.
                reason: "reserved OKF names like index.md and log.md are never imported"
                    .to_string(),
            });
            continue;
        }
        // The header's uncompressed size is a CLAIM, not a bound: zip 8.6.0
        // decompresses past it happily, and deflate expands up to ~1032:1, so
        // a lying header would otherwise allow multi-GiB allocations from a
        // 10 MiB body. Refuse the obvious oversize cheaply, then meter the
        // ACTUAL decompressed bytes through a capped reader.
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(ApiError::unprocessable(format!(
                "archive entry '{raw}' declares {} bytes uncompressed; the per-entry limit is {MAX_ENTRY_BYTES}",
                entry.size()
            )));
        }
        let mut buf = Vec::new();
        {
            use std::io::Read;
            let mut limited = (&mut entry).take(MAX_ENTRY_BYTES + 1);
            limited.read_to_end(&mut buf).map_err(|e| {
                ApiError::unprocessable(format!("unreadable archive entry '{raw}': {e}"))
            })?;
        }
        if buf.len() as u64 > MAX_ENTRY_BYTES {
            return Err(ApiError::unprocessable(format!(
                "archive entry '{raw}' decompresses past the per-entry limit of {MAX_ENTRY_BYTES} bytes"
            )));
        }
        total += buf.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(ApiError::unprocessable(format!(
                "the archive unpacks past the {MAX_TOTAL_BYTES}-byte total limit"
            )));
        }
        match String::from_utf8(buf) {
            Ok(content) => out.push(Screened::Entry { path: raw, content }),
            Err(_) => out.push(Screened::Invalid {
                path: raw,
                reason: "not UTF-8 text".to_string(),
            }),
        }
    }
    Ok((out, loose))
}

/// An entry after it was completed and verified, before anything is
/// written.
enum Pending {
    /// Already decided.
    Decided(ArchiveEntryReport),
    /// An `assets/` entry for the attachment seam.
    Asset { path: String, bytes: Vec<u8> },
    /// A file outside `assets/` that is not markdown: stored at its
    /// attachment path when a note links it, ignored for `reason` otherwise.
    Loose { path: String, reason: String },
    /// A note that passed verify, waiting for its links to be settled.
    Note {
        path: String,
        findings: Vec<ValidateFinding>,
    },
}

/// A report line, either already decided by the screen or waiting for the
/// engine's verdict on the entry that was handed to it.
enum Slot {
    Decided(ArchiveEntryReport),
    Sent {
        path: String,
        findings: Vec<ValidateFinding>,
    },
}

/// The engine's action word as this surface reports it. The dry-run flag is
/// what separates "would be created" from "was created": one engine verb
/// backs both calls, so the vocabulary is translated here rather than there.
fn status_of(action: &str, dry_run: bool) -> String {
    match action {
        "create" => {
            if dry_run {
                "new"
            } else {
                "created"
            }
        }
        // A preview runs under the skip policy, so an existing path comes back
        // as `skip` there; `overwrite` can only appear on a committing import.
        "overwrite" => {
            if dry_run {
                "collides"
            } else {
                "overwritten"
            }
        }
        "skip" => {
            if dry_run {
                "collides"
            } else {
                "skipped"
            }
        }
        // `invalid` and `ignored` mean the same on both sides of the flag, and
        // an action this layer has never seen is passed through verbatim
        // rather than renamed into something reassuring.
        other => other,
    }
    .to_string()
}

/// Screen, verify, then classify through the engine - the whole of both
/// handlers, so a preview and the import that follows it run one code path
/// with one flag flipped.
///
/// Entries carrying an Error-severity finding are withheld from the engine
/// under either policy: the preview called them invalid, so the import must
/// not write them. Preview passes `overwrite = false`, because a collision is
/// reported rather than resolved there and the policy choice belongs to the
/// call that commits.
async fn run_archive(
    state: &RestState,
    identity: &Identity,
    domain: &str,
    bytes: Bytes,
    overwrite: bool,
    dry_run: bool,
) -> Result<ArchiveReport, ApiError> {
    // Resolve the domain before paying for decompression: `read_archive`
    // meters up to MAX_TOTAL_BYTES of decompressed content, and an upload
    // aimed at a domain that does not exist can never succeed regardless of
    // what is inside it. Both `preview` and `import` inherit this order from
    // this one function. It still sits BELOW `identity.require_admin()` and
    // `refuse_read_only` in both handlers - moving it above them would make a
    // read-only instance answer 404 where it answers 403, and let an
    // unauthorized caller learn which domains exist.
    state
        .engine
        .require_domain(domain, &identity.scope())
        .await?;
    // Before the decompression and well before the attachment arm below, which
    // writes through the engine's own upload seam: a domain that reviews
    // changes refuses an import, and a refusal that arrived after half the zip
    // had landed would be a rule enforced too late to mean anything. The
    // preview is exempt for the reason the engine's own check states - it
    // writes nothing.
    if !dry_run {
        state
            .engine
            .refuse_write_into_reviewed_folder(domain, "an imported archive cannot land there")?;
    }
    // On the blocking pool, like `build_zip` on the way out: inflating an
    // archive is synchronous CPU work bounded by MAX_TOTAL_BYTES, and running
    // it inline would park a runtime worker for the whole of it. `Bytes` is
    // cheap to move here - it hands the shared buffer over rather than copying
    // the body.
    let (screened, mut loose) = tokio::task::spawn_blocking(move || read_archive(&bytes))
        .await
        .map_err(|e| ApiError::internal(format!("archive screening task failed: {e}")))??;
    // What the domain already holds under `assets/`, by path and sha256, so
    // an attachment entry gets the same same-path treatment a markdown entry
    // does (reported as a collision by a preview, left alone by a skip-policy
    // import, replaced only under overwrite) and a linked file is stored
    // once. Fetched once, and only when the archive carries a file that could
    // become an attachment.
    let mut held: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if !loose.is_empty()
        || screened
            .iter()
            .any(|item| matches!(item, Screened::Asset { .. }))
    {
        // The importer's own view: a preview compares what the archive carries
        // against what the person running the import already sees, so a file
        // they hold only in their own overlay collides for them the way a
        // reviewed one collides for everybody.
        let scope = identity.scope();
        let hidden = state.engine.hidden_for(&scope).await?;
        held = DomainView::for_read(&state.engine, domain, &hidden, &scope)?
            .attachments()
            .await?
            .into_iter()
            .map(|row| (row.path, row.sha256))
            .collect();
    }

    // Pass 1: complete every markdown entry, the way `crystalline import`
    // completes a file, and only then verify it, so a note that lacks only
    // what completion adds is not refused for it. The notes that pass are
    // what the linked-file step reads.
    let now = chrono::Utc::now().fixed_offset();
    let mut pending: Vec<Pending> = Vec::new();
    let mut candidates: Vec<(String, String)> = Vec::new();
    let mut notes: Vec<crystalline_core::linked::LinkedNote> = Vec::new();
    for item in screened {
        match item {
            Screened::Ignored { path, reason } if loose.contains(&path) => {
                pending.push(Pending::Loose { path, reason });
            }
            Screened::Ignored { path, reason } => {
                pending.push(Pending::Decided(ArchiveEntryReport {
                    path,
                    status: "ignored".to_string(),
                    permalink: None,
                    reason: Some(reason),
                    bytes: None,
                    findings: Vec::new(),
                }))
            }
            Screened::Invalid { path, reason } => {
                pending.push(Pending::Decided(ArchiveEntryReport {
                    path,
                    status: "invalid".to_string(),
                    permalink: None,
                    reason: Some(reason),
                    bytes: None,
                    findings: Vec::new(),
                }))
            }
            Screened::Asset { path, bytes } => pending.push(Pending::Asset { path, bytes }),
            Screened::Entry { path, content } => {
                // A note that does not parse is verified as it is, so the
                // findings name the reason.
                let content =
                    match crystalline_core::import::complete_for_import(&content, &path, now) {
                        Ok(Some(completed)) => completed,
                        _ => content,
                    };
                // The exact call `/validate` makes, on the path the entry
                // would land at, so the findings an editor sees before a save
                // and the findings a preview shows are the same findings.
                let findings =
                    super::engrams::findings_of(crystalline_core::verify::check_document(
                        domain,
                        std::path::Path::new(&path),
                        &content,
                    ));
                if let Some(reason) = findings
                    .iter()
                    .find(|f| f.is_error())
                    .map(ValidateFinding::summary)
                {
                    pending.push(Pending::Decided(ArchiveEntryReport {
                        path,
                        status: "invalid".to_string(),
                        permalink: None,
                        reason: Some(reason),
                        bytes: None,
                        findings,
                    }));
                } else {
                    candidates.push((path.clone(), content));
                    pending.push(Pending::Note { path, findings });
                }
            }
        }
    }

    // A note the import will skip (its path is taken under the skip policy,
    // or its permalink is held at another path) links nothing into the
    // domain: its files are not taken, so no attachment lands that nothing
    // links. Asked of the engine's own classification in a preview, which
    // decides by path and permalink alone, so it answers what the import
    // below will do.
    let mut held_back: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if !candidates.is_empty() {
        let preview = state
            .engine
            .import_domain_files(domain, &candidates, overwrite, true)
            .await?;
        let skipped: std::collections::HashSet<String> = preview["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["action"] == "skip")
            .filter_map(|row| row["path"].as_str().map(str::to_string))
            .collect();
        for (path, text) in candidates {
            if skipped.contains(&path) {
                held_back.insert(path, text);
            } else {
                notes.push(crystalline_core::linked::LinkedNote { rel: path, text });
            }
        }
    }

    // Pass 2: take the files the notes link to. The archive's own `assets/`
    // entries are written in the pass below too, so a dest one of them
    // claims will hold its bytes, unless a skip-policy import leaves the
    // attachment already there in place.
    let archive_assets: std::collections::HashMap<String, String> = pending
        .iter()
        .filter_map(|item| match item {
            Pending::Asset { path, bytes } => Some((
                path.clone(),
                crystalline_index::hex_lower(&<sha2::Sha256 as sha2::Digest>::digest(bytes)),
            )),
            _ => None,
        })
        .collect();
    let linked = crystalline_core::linked::take_linked_files(&mut notes, &loose, &|dest| match (
        held.get(dest),
        archive_assets.get(dest),
    ) {
        (Some(kept), _) if !overwrite => Some(kept.clone()),
        (kept, incoming) => incoming.or(kept).cloned(),
    });
    let taken: std::collections::HashMap<&str, &crystalline_core::linked::TakenFile> = linked
        .taken
        .iter()
        .map(|file| (file.source_rel.as_str(), file))
        .collect();
    let mut rewritten = notes.into_iter();

    // Pass 3: write every attachment, the archive's own and the linked ones,
    // before any note that links one is handed to the engine.
    let mut slots: Vec<Slot> = Vec::new();
    let mut clean: Vec<(String, String)> = Vec::new();
    // The linked files whose write failed, with why: a note that links one
    // is not imported, so no note lands with a link to a file that is not
    // there.
    let mut unstored: Vec<(String, String)> = Vec::new();
    for item in pending {
        match item {
            Pending::Decided(report) => slots.push(Slot::Decided(report)),
            // Attachments go through the engine's own seam - the same one the
            // upload route uses - so the path rules, the allowlist, the size
            // ceiling and the metadata row are decided in exactly one place.
            // They are written here rather than handed to the engine's import
            // verb because that verb is about engrams: a permalink, a parse and
            // a verify pass, none of which a PNG has.
            Pending::Asset { path, bytes } => {
                let size = bytes.len() as u64;
                let sha = archive_assets.get(&path).cloned().unwrap_or_default();
                let taken = held.contains_key(&path);
                let (status, reason) = if dry_run {
                    held.insert(path.clone(), sha);
                    (if taken { "collides" } else { "new" }.to_string(), None)
                } else if taken && !overwrite {
                    (
                        "skipped".to_string(),
                        Some(
                            "the attachment is already there; import with policy=overwrite to \
                             replace it"
                                .to_string(),
                        ),
                    )
                } else {
                    match state.engine.attachment_write(domain, &path, bytes).await {
                        Ok(_) => {
                            held.insert(path.clone(), sha);
                            (
                                if taken { "overwritten" } else { "created" }.to_string(),
                                None,
                            )
                        }
                        // Whatever the seam refused it for, in its own words:
                        // this layer screened the path, the extension and the
                        // size already, so anything left is the storage
                        // failing rather than the entry being wrong.
                        Err(e) => ("invalid".to_string(), Some(e.to_string())),
                    }
                };
                slots.push(Slot::Decided(ArchiveEntryReport {
                    path,
                    status,
                    permalink: None,
                    reason,
                    bytes: Some(size),
                    findings: Vec::new(),
                }));
            }
            // A file a note links is listed at the attachment path it is
            // stored at, the way an `assets/` entry is. Bytes the domain
            // already holds there are not written again.
            Pending::Loose { path, reason } => {
                let Some(file) = taken.get(path.as_str()) else {
                    // The same bytes as a file already taken: every link to
                    // it points at that file's attachment, which is listed
                    // on its own row; this one says which.
                    if linked.linked_sources.contains(&path) {
                        let bytes = loose.read.files.remove(&path).unwrap_or_default();
                        let sha = crystalline_index::hex_lower(
                            &<sha2::Sha256 as sha2::Digest>::digest(&bytes),
                        );
                        let shared = linked
                            .taken
                            .iter()
                            .find(|file| file.sha256 == sha)
                            .map_or_else(String::new, |file| file.dest.clone());
                        slots.push(Slot::Decided(ArchiveEntryReport {
                            path,
                            status: "skipped".to_string(),
                            permalink: None,
                            reason: Some(format!("{SAME_BYTES_REASON} {shared}")),
                            bytes: Some(bytes.len() as u64),
                            findings: Vec::new(),
                        }));
                        continue;
                    }
                    slots.push(Slot::Decided(ArchiveEntryReport {
                        path,
                        status: "ignored".to_string(),
                        permalink: None,
                        reason: Some(reason),
                        bytes: None,
                        findings: Vec::new(),
                    }));
                    continue;
                };
                let bytes = loose.read.files.remove(&path).unwrap_or_default();
                let size = bytes.len() as u64;
                let (status, reason) = if file.reused {
                    (
                        if dry_run { "collides" } else { "skipped" }.to_string(),
                        Some(SAME_FILE_REASON.to_string()),
                    )
                } else if dry_run {
                    ("new".to_string(), None)
                } else {
                    match state
                        .engine
                        .attachment_write(domain, &file.dest, bytes)
                        .await
                    {
                        Ok(_) => ("created".to_string(), None),
                        Err(e) => {
                            unstored.push((file.dest.clone(), e.to_string()));
                            ("invalid".to_string(), Some(e.to_string()))
                        }
                    }
                };
                slots.push(Slot::Decided(ArchiveEntryReport {
                    path: file.dest.clone(),
                    status,
                    permalink: None,
                    reason,
                    bytes: Some(size),
                    findings: Vec::new(),
                }));
            }
            Pending::Note { path, findings } => {
                let text = match held_back.remove(&path) {
                    Some(text) => text,
                    None => {
                        rewritten
                            .next()
                            .ok_or_else(|| {
                                ApiError::internal(format!("no linked-file outcome for '{path}'"))
                            })?
                            .text
                    }
                };
                slots.push(Slot::Sent {
                    path: path.clone(),
                    findings,
                });
                clean.push((path, text));
            }
        }
    }

    // A note that links a file whose write failed is reported invalid and
    // not handed over, as `crystalline import` stops on that error.
    if !unstored.is_empty() {
        let failed_for = |text: &str| {
            unstored
                .iter()
                .find(|(dest, _)| crystalline_core::find_asset_refs(text).contains(dest))
                .map(|(dest, e)| format!("the file it links could not be stored at {dest}: {e}"))
        };
        let reasons: std::collections::HashMap<String, String> = clean
            .iter()
            .filter_map(|(path, text)| failed_for(text).map(|reason| (path.clone(), reason)))
            .collect();
        clean.retain(|(path, _)| !reasons.contains_key(path));
        for slot in &mut slots {
            if let Slot::Sent { path, findings } = slot
                && let Some(reason) = reasons.get(path.as_str())
            {
                *slot = Slot::Decided(ArchiveEntryReport {
                    path: path.clone(),
                    status: "invalid".to_string(),
                    permalink: None,
                    reason: Some(reason.clone()),
                    bytes: None,
                    findings: std::mem::take(findings),
                });
            }
        }
    }

    let outcome = state
        .engine
        .import_domain_files(domain, &clean, overwrite, dry_run)
        .await?;
    let mut rows = outcome["files"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter();

    let mut entries: Vec<ArchiveEntryReport> = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Slot::Decided(report) => entries.push(report),
            Slot::Sent { path, findings } => {
                // One row per handed-over file, in the order they were handed
                // over: the engine reports every entry it was given.
                let row = rows.next().ok_or_else(|| {
                    ApiError::internal(format!("the import reported no outcome for '{path}'"))
                })?;
                entries.push(ArchiveEntryReport {
                    path,
                    status: status_of(row["action"].as_str().unwrap_or("invalid"), dry_run),
                    permalink: row["permalink"].as_str().map(str::to_string),
                    reason: row["reason"].as_str().map(str::to_string),
                    bytes: None,
                    findings,
                });
            }
        }
    }

    // Counted from the lines above rather than from the engine's own totals:
    // this report also carries the entries the engine never saw, and a
    // counter that disagreed with the list under it would be the one thing a
    // confirmation dialog cannot recover from.
    let count = |want: &str| entries.iter().filter(|e| e.status == want).count();
    Ok(ArchiveReport {
        domain: domain.to_string(),
        dry_run,
        new: count("new"),
        collides: count("collides"),
        written: count("created") + count("overwritten"),
        skipped: count("skipped"),
        invalid: count("invalid"),
        ignored: count("ignored"),
        files_linked: linked.taken.len(),
        not_taken: linked
            .not_taken
            .into_iter()
            .map(ArchiveNotTaken::from)
            .collect(),
        entries,
    })
}

/// `POST /domains/{domain}/archive/preview` - what an import would do, without
/// doing any of it.
///
/// The dry run of the same engine verb [`import`] commits with, so the report
/// a user approves is the report the import produces. Refused on a read-only
/// instance even though it writes nothing: it is the first half of a write,
/// and letting it through would offer an operator a button whose second half
/// can only fail.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/archive/preview",
    tag = "domains",
    operation_id = "preview_domain_archive_import",
    summary = "Dry-run an archive upload: what each entry would become.",
    description = "Admin only. Takes the raw bytes of a zip and reports, per \
                   entry, what an import would do with it - `new`, \
                   `collides`, `invalid` or `ignored` - with the verify \
                   findings `POST /validate` would raise over that entry's \
                   markdown.\n\nEntries under `assets/` are attachments rather \
                   than engrams: they are reported with their byte count and \
                   screened by the attachment path rules, the extension \
                   allowlist and the size ceiling, each refusal naming which \
                   of the three answered; an attachment past the 10 MiB \
                   ceiling is refused on its own rather than refusing the \
                   archive.\n\nEach markdown entry is completed the \
                   way `crystalline import` completes a file before it is \
                   verified. A file of the archive outside `assets/` that a \
                   note links is listed at the `assets/` path it would be \
                   stored at, the link is rewritten to it, and every link \
                   that cannot be taken is listed in `not_taken`; a file no \
                   note links stays `ignored`.\n\nNothing is written. A hostile archive is \
                   refused whole with 422 rather than partially imported: \
                   more than 1000 entries, a markdown entry over 1 MiB or a \
                   whole archive over 200 MiB once decompressed, an entry \
                   name that is not UTF-8, or any path that could escape the \
                   domain root.",
    params(("domain" = String, Path, description = "The registered domain the archive is aimed at.")),
    request_body(
        // Raw zip bytes. Declared as a string for the same reason the download
        // declares its response one: utoipa renders `Vec<u8>` as an array of
        // integers, and the content type is what carries the meaning here.
        content = String,
        description = "The raw bytes of a zip archive.",
        content_type = "application/zip",
    ),
    responses(
        (
            status = 200,
            description = "The per-entry report, and the counters under it.",
            body = ArchiveReport,
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "The upload is past the 64 MiB body limit these \
                           two archive routes accept, which is deliberately \
                           larger than the 10 MiB the rest of this API takes.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The bytes are not a readable zip, or the archive \
                           fails hygiene - the detail names which rule.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn preview(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    // Last, and it has to be: `Bytes` consumes the body. The limit it is read
    // under is this route's own `DefaultBodyLimit`
    // ([`super::ARCHIVE_BODY_BYTES`], 64 MiB, layered inside the surface's
    // general 10 MiB one, and axum takes the innermost), so an upload past 64
    // MiB is refused with 413 before this handler runs.
    body: Bytes,
) -> Result<Json<ArchiveReport>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    Ok(Json(
        run_archive(&state, &identity, &domain, body, false, true).await?,
    ))
}

/// `POST /domains/{domain}/archive/import?policy=skip|overwrite` - commit the
/// upload the preview described.
///
/// `overwrite` is a SAME-PATH decision only: an entry whose permalink is
/// already held at a different path is refused under both policies by the
/// engine, because writing it would leave two files claiming one permalink.
#[utoipa::path(
    post,
    path = "/api/v1/domains/{domain}/archive/import",
    tag = "domains",
    operation_id = "import_domain_archive",
    summary = "Import an uploaded archive into a domain.",
    description = "Admin only. Runs the engine verb the preview dry-ran, so \
                   the outcome matches the report that was approved: entries \
                   land as `created` or `overwritten`, and `skipped`, \
                   `invalid` and `ignored` name everything that did \
                   not.\n\n`policy=skip` (the default) leaves an existing path \
                   alone; `policy=overwrite` replaces it. Overwrite is a \
                   same-path decision only - an entry whose permalink is held \
                   at another path is refused under either policy, since \
                   writing it would leave two files claiming one \
                   permalink.\n\nEntries under `assets/` are stored as \
                   attachments, through the same gates an upload to the file \
                   route passes, and follow the same same-path policy; a \
                   restored attachment marks the domain as owing a \
                   consolidation sweep, since it is a file the agent has not \
                   read yet.\n\nMarkdown entries are completed and the \
                   files they link are stored as attachments, as the preview \
                   describes; a linked file whose bytes the domain already \
                   holds at its path is not written again.\n\nThe same hygiene the preview enforces applies \
                   here: a hostile archive is refused whole rather than \
                   partially imported.",
    params(
        ("domain" = String, Path, description = "The registered domain to import into."),
        ImportQuery,
    ),
    request_body(
        content = String,
        description = "The raw bytes of a zip archive.",
        content_type = "application/zip",
    ),
    responses(
        (
            status = 200,
            description = "The per-entry report of what landed.",
            body = ArchiveReport,
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "The upload is past the 64 MiB body limit these \
                           two archive routes accept, which is deliberately \
                           larger than the 10 MiB the rest of this API takes.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The bytes are not a readable zip, the archive fails \
                           hygiene, or `policy` is neither `skip` nor \
                           `overwrite`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn import(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<ImportQuery>,
    // Last for the same reason as in [`preview`]: `Bytes` consumes the body.
    body: Bytes,
) -> Result<Json<ArchiveReport>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    state
        .engine
        .require_domain(&domain, &identity.scope())
        .await?;
    crate::refuse_off_write_list(&state, &identity, &domain, None).await?;
    // An unknown policy is refused rather than treated as the safe default: a
    // client that asked for something this endpoint does not do must hear so,
    // not silently get a skip it never requested.
    let overwrite = match query.policy.as_deref() {
        None | Some("skip") => false,
        Some("overwrite") => true,
        Some(other) => {
            return Err(ApiError::unprocessable(format!(
                "policy must be skip or overwrite, got '{other}'"
            )));
        }
    };
    Ok(Json(
        run_archive(&state, &identity, &domain, body, overwrite, false).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filename_cannot_carry_a_path_or_a_quote() {
        assert_eq!(archive_filename("eng"), "eng-archive.zip");
        assert_eq!(archive_filename("a/b"), "a-b-archive.zip");
        assert_eq!(archive_filename("../up"), "..-up-archive.zip");
        assert_eq!(archive_filename("we\"ird\r\n"), "we-ird---archive.zip");
        assert_eq!(archive_filename("wissen-über"), "wissen--ber-archive.zip");
    }

    /// A PNG stand-in: it never has to decode, only to survive the zip
    /// unchanged, so it is a short blob carrying the NUL a text-shaped path
    /// would lose.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00binary\x00bytes";

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            for (name, content) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// The screened verdict for one entry, as `(status word, reason)`.
    fn verdict(screened: &Screened) -> (&'static str, Option<&str>) {
        match screened {
            Screened::Entry { .. } => ("entry", None),
            Screened::Asset { .. } => ("asset", None),
            Screened::Ignored { reason, .. } => ("ignored", Some(reason.as_str())),
            Screened::Invalid { reason, .. } => ("invalid", Some(reason.as_str())),
        }
    }

    #[test]
    fn the_zip_carries_every_file_verbatim() {
        let files = vec![
            ("MANIFEST.md".to_string(), b"# eng\n".to_vec()),
            ("sub/alpha.md".to_string(), b"body\n".to_vec()),
            ("assets/shot.png".to_string(), PNG.to_vec()),
        ];
        let bytes = build_zip(&files).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 3);
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("sub/alpha.md").unwrap(), &mut text)
            .unwrap();
        assert_eq!(text, "body\n");
        let mut png = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_name("assets/shot.png").unwrap(), &mut png)
            .unwrap();
        assert_eq!(png, PNG, "the binary survives the archive byte for byte");
    }

    /// The name screen gives the same answers on every OS: a name zip says
    /// this OS cannot hold (on Windows since zip 9.1: a component over 255
    /// UTF-16 units, a device name, a trailing dot) is not an escape. An
    /// `assets/` name is answered by the attachment gate with its own reason,
    /// any other by a reason of its own. A real escape refuses the archive
    /// whatever zip said.
    #[test]
    fn a_name_this_os_cannot_hold_is_not_an_escape() {
        let named = |raw: &str, enclosed: bool| {
            entry_name_verdict(raw, enclosed).unwrap().map(|v| {
                let (status, reason) = verdict(&v);
                (status, reason.map(str::to_string))
            })
        };
        // Where zip takes the long name (every OS but Windows) the assets arm
        // answers it later, through the same gate:
        // `asset_entries_are_screened_by_the_attachment_gates` pins that.
        let long_asset = format!("assets/{}.png", "n".repeat(300));
        assert_eq!(named(&long_asset, true), None);
        assert_eq!(
            named(&long_asset, false),
            Some(("invalid", Some(ASSET_PATH_REASON.to_string()))),
            "the attachment gate names the long asset zip refused"
        );
        assert_eq!(named("assets/shot.png", true), None);
        assert_eq!(
            named("assets/shot.png", false),
            Some(("invalid", Some(NOT_A_FILE_NAME_HERE_REASON.to_string()))),
            "a good asset name zip refuses still gets a verdict, never an escape"
        );
        let long_note = format!("notes/{}.md", "n".repeat(300));
        assert_eq!(named(&long_note, true), None);
        assert_eq!(
            named(&long_note, false),
            Some(("invalid", Some(NOT_A_FILE_NAME_HERE_REASON.to_string())))
        );

        for escape in [
            "../x.md",
            "a/../../x.md",
            "a/./x.md",
            "a//x.md",
            "/etc/x.md",
            "//server/share/x.md",
            "C:/x.md",
            "C:x.md",
            "a/file.md:stream",
            r"\\server\share\x.md",
            r"a\..\..\x.md",
            r"assets\x.png",
            "a/x\0.md",
        ] {
            for enclosed in [true, false] {
                let Err(err) = entry_name_verdict(escape, enclosed) else {
                    panic!("{escape:?} (enclosed {enclosed}) is an escape");
                };
                assert_eq!(err.status.as_u16(), 422, "{escape:?}");
                assert!(
                    err.detail.contains("escapes the extraction root"),
                    "{escape:?}: {}",
                    err.detail
                );
            }
        }
    }

    /// An `assets/` entry is screened by the upload gates rather than by the
    /// markdown rule: a good one is kept as an attachment, and every refusal
    /// names which gate answered.
    #[test]
    fn asset_entries_are_screened_by_the_attachment_gates() {
        let long = format!("assets/{}.png", "n".repeat(300));
        let bytes = zip_of(&[
            ("assets/shot.png", PNG),
            ("assets/tool.exe", b"MZ"),
            ("assets/notes.md", b"# not an attachment"),
            ("assets/.hidden.png", PNG),
            (long.as_str(), PNG),
            ("readme.txt", b"not markdown"),
        ]);
        let (screened, _) = read_archive(&bytes).unwrap();
        let by_path = |path: &str| -> (&'static str, Option<String>) {
            let found = screened
                .iter()
                .find(|s| match s {
                    Screened::Entry { path: p, .. }
                    | Screened::Asset { path: p, .. }
                    | Screened::Ignored { path: p, .. }
                    | Screened::Invalid { path: p, .. } => p == path,
                })
                .unwrap_or_else(|| panic!("no verdict for {path}"));
            let (status, reason) = verdict(found);
            (status, reason.map(str::to_string))
        };

        assert_eq!(by_path("assets/shot.png"), ("asset", None));
        assert_eq!(
            by_path("assets/tool.exe"),
            ("ignored", Some(ASSET_EXTENSION_REASON.to_string()))
        );
        assert_eq!(
            by_path("assets/notes.md"),
            ("ignored", Some(ASSET_MARKDOWN_REASON.to_string())),
            "an assets/ markdown entry is neither an engram nor an attachment"
        );
        assert_eq!(
            by_path("assets/.hidden.png"),
            ("invalid", Some(ASSET_PATH_REASON.to_string()))
        );
        assert_eq!(
            by_path(&long),
            ("invalid", Some(ASSET_PATH_REASON.to_string()))
        );
        // The markdown rule is untouched for everything outside assets/.
        assert_eq!(
            by_path("readme.txt"),
            ("ignored", Some("only .md entries are imported".to_string()))
        );

        // The bytes of the kept attachment are the bytes the archive held.
        let kept = screened
            .iter()
            .find_map(|s| match s {
                Screened::Asset { path, bytes } if path == "assets/shot.png" => Some(bytes),
                _ => None,
            })
            .unwrap();
        assert_eq!(kept.as_slice(), PNG);
    }

    /// An attachment past the ceiling is one refused entry, not a refused
    /// archive: the whole-archive hygiene rules stay for the shapes that are
    /// hostile rather than merely too big.
    #[test]
    fn an_oversized_asset_entry_is_refused_on_its_own() {
        let big = vec![b'a'; crystalline_core::MAX_ATTACHMENT_BYTES as usize + 1];
        let bytes = zip_of(&[("assets/huge.png", big.as_slice())]);
        let (screened, _) = read_archive(&bytes).unwrap();
        assert_eq!(
            verdict(&screened[0]),
            ("invalid", Some(ASSET_SIZE_REASON)),
            "the size gate answers per entry"
        );
    }

    /// A non-markdown entry outside `assets/` is still reported as ignored,
    /// and is kept for the notes that may link it: its bytes when they could
    /// become an attachment, its size alone when its type or its size rules
    /// that out, so the planner can say why without the bytes being held.
    #[test]
    fn loose_entries_are_kept_for_the_notes_that_link_them() {
        use crystalline_core::linked::{LinkedSource, Lookup};
        let big = vec![b'a'; crystalline_core::MAX_ATTACHMENT_BYTES as usize + 1];
        let bytes = zip_of(&[
            ("pic.png", PNG),
            ("tool.exe", b"MZ"),
            ("huge.png", big.as_slice()),
        ]);
        let (screened, loose) = read_archive(&bytes).unwrap();
        for item in &screened {
            assert_eq!(
                verdict(item),
                ("ignored", Some("only .md entries are imported"))
            );
        }
        assert_eq!(
            loose.lookup("pic.png"),
            Lookup::File {
                len: PNG.len() as u64
            }
        );
        assert!(loose.sha256("pic.png").is_ok());
        assert_eq!(loose.lookup("tool.exe"), Lookup::File { len: 2 });
        assert!(loose.sha256("tool.exe").is_err(), "its bytes are not held");
        assert_eq!(
            loose.lookup("huge.png"),
            Lookup::File {
                len: big.len() as u64
            }
        );
        assert!(loose.sha256("huge.png").is_err(), "its bytes are not held");
        assert_eq!(loose.lookup("absent.png"), Lookup::Missing);
    }

    /// A markdown entry keeps the 1 MiB per-entry cap it always had: the wider
    /// attachment ceiling belongs to `assets/` alone.
    #[test]
    fn a_markdown_entry_keeps_its_own_cap() {
        let big = vec![b'a'; MAX_ENTRY_BYTES as usize + 1];
        let bytes = zip_of(&[("big.md", big.as_slice())]);
        let Err(refused) = read_archive(&bytes) else {
            panic!("an oversized markdown entry refuses the archive");
        };
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
