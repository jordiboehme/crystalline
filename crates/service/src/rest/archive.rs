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
//! - A *per-entry* demotion - a non-`.md` file, a MANIFEST, an OKF reserved
//!   name, bytes that are not UTF-8 text, a hard verify error - stays in the
//!   report as that entry's status and never reaches the engine.
//!
//! What this layer screens, and what it leaves to the engine: the screen here
//! covers zip-slip, the size and count caps, non-`.md` entries, MANIFEST at
//! any depth and the OKF reserved names (`index.md`, `log.md`). The engine's
//! `import_domain_files` screens the same MANIFEST, non-markdown and reserved
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

use super::auth::Identity;
use super::engrams::ValidateFinding;
use super::{ApiError, ApiPath, ApiQuery, ProblemDetail, RestState, refuse_read_only};

/// How many entries an uploaded archive may hold.
const MAX_ARCHIVE_ENTRIES: usize = 1000;

/// The largest a single entry may be once decompressed, in bytes.
const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

/// The largest a whole archive may be once decompressed, in bytes.
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

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
    description = "Admin only. Every file of the domain - MANIFEST included - \
                   as one zip, read from whichever source of truth the domain \
                   has: markdown on disk for a file domain, the database for \
                   a virtual one. A pure read, so it is served even on a \
                   read-only instance, which is exactly where an operator \
                   wants a backup to take.",
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
fn build_zip(files: &[(String, String)]) -> Result<Vec<u8>, zip::result::ZipError> {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        for (path, content) in files {
            writer.start_file(path.as_str(), options)?;
            writer.write_all(content.as_bytes())?;
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
    /// Entries refused as not importable: unparseable, not UTF-8 text, or
    /// carrying a hard verify error. Never written under either policy.
    pub invalid: usize,
    /// Entries an archive may carry but a domain never imports: a MANIFEST, a
    /// generated OKF index or log, anything that is not markdown.
    pub ignored: usize,
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
    /// An entry a domain never imports, whatever it holds.
    Ignored { path: String, reason: String },
    /// An entry that cannot be imported as it stands.
    Invalid { path: String, reason: String },
}

/// Open and screen an uploaded archive.
///
/// Hygiene refusals (not a zip, too many entries, oversized by declaration OR
/// by actual decompressed bytes, a name that is not UTF-8, traversal) reject
/// the WHOLE request: a hostile archive gets no partial import. Per-entry
/// demotions (non-`.md`, MANIFEST, reserved names, non-UTF-8 content) stay in
/// the report. Size limits are enforced on the bytes actually decompressed,
/// never on the header's claim alone.
fn read_archive(bytes: &[u8]) -> Result<Vec<Screened>, ApiError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ApiError::unprocessable(format!("not a readable zip archive: {e}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ApiError::unprocessable(format!(
            "the archive has {} entries; the limit is {MAX_ARCHIVE_ENTRIES}",
            archive.len()
        )));
    }
    let mut total: u64 = 0;
    let mut out = Vec::new();
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
        let raw = entry.name().to_string();
        // Zip-slip and Windows-hostile shapes: zip's own containment check
        // plus the component rules the remote crate's to_platform_path
        // enforces (no empty/'.'/'..' components, no ':' or '\\' anywhere).
        let contained = entry.enclosed_name().is_some();
        let clean_components = !raw.starts_with('/')
            && raw.split('/').all(|c| {
                !c.is_empty() && c != "." && c != ".." && !c.contains(':') && !c.contains('\\')
            });
        if !contained || !clean_components {
            return Err(ApiError::unprocessable(format!(
                "archive entry '{raw}' escapes the extraction root; refusing the archive"
            )));
        }
        // Entries this endpoint will never decompress are classified before
        // any size handling: no read, no allocation.
        if !raw.to_lowercase().ends_with(".md") {
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
            .is_some_and(|name| name.eq_ignore_ascii_case("MANIFEST.md"))
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
    Ok(out)
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
    domain: &str,
    bytes: &[u8],
    overwrite: bool,
    dry_run: bool,
) -> Result<ArchiveReport, ApiError> {
    // Resolve the domain before paying for decompression: `read_archive`
    // meters up to 32 MiB of decompressed content, and an upload aimed at a
    // domain that does not exist can never succeed regardless of what is
    // inside it. Both `preview` and `import` inherit this order from this one
    // function. It still sits BELOW `identity.require_admin()` and
    // `refuse_read_only` in both handlers - moving it above them would make a
    // read-only instance answer 404 where it answers 403, and let an
    // unauthorized caller learn which domains exist.
    state.engine.require_domain(domain)?;
    let screened = read_archive(bytes)?;
    let mut slots: Vec<Slot> = Vec::new();
    let mut clean: Vec<(String, String)> = Vec::new();
    for item in screened {
        match item {
            Screened::Ignored { path, reason } => slots.push(Slot::Decided(ArchiveEntryReport {
                path,
                status: "ignored".to_string(),
                permalink: None,
                reason: Some(reason),
                findings: Vec::new(),
            })),
            Screened::Invalid { path, reason } => slots.push(Slot::Decided(ArchiveEntryReport {
                path,
                status: "invalid".to_string(),
                permalink: None,
                reason: Some(reason),
                findings: Vec::new(),
            })),
            Screened::Entry { path, content } => {
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
                    slots.push(Slot::Decided(ArchiveEntryReport {
                        path,
                        status: "invalid".to_string(),
                        permalink: None,
                        reason: Some(reason),
                        findings,
                    }));
                } else {
                    slots.push(Slot::Sent {
                        path: path.clone(),
                        findings,
                    });
                    clean.push((path, content));
                }
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
                   markdown.\n\nNothing is written. A hostile archive is \
                   refused whole with 422 rather than partially imported: \
                   more than 1000 entries, an entry over 1 MiB or a whole \
                   archive over 32 MiB once decompressed, an entry name that \
                   is not UTF-8, or any path that could escape the domain \
                   root.",
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
            description = "The upload is past the surface's request-body limit.",
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
    // Last, and it has to be: `Bytes` consumes the body. It rides the
    // surface's outermost `DefaultBodyLimit`, so an oversized upload is
    // refused with 413 before this handler runs.
    body: Bytes,
) -> Result<Json<ArchiveReport>, ApiError> {
    identity.require_admin()?;
    refuse_read_only(&state)?;
    Ok(Json(
        run_archive(&state, &domain, &body, false, true).await?,
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
                   permalink.\n\nThe same hygiene the preview enforces applies \
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
            description = "The upload is past the surface's request-body limit.",
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
        run_archive(&state, &domain, &body, overwrite, false).await?,
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

    #[test]
    fn the_zip_carries_every_file_verbatim() {
        let files = vec![
            ("MANIFEST.md".to_string(), "# eng\n".to_string()),
            ("sub/alpha.md".to_string(), "body\n".to_string()),
        ];
        let bytes = build_zip(&files).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("sub/alpha.md").unwrap(), &mut text)
            .unwrap();
        assert_eq!(text, "body\n");
    }
}
