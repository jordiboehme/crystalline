//! Attachment bytes: serving, uploading and deleting the files an engram
//! carries, plus the metadata listing a file browser renders.
//!
//! Everything that decides what an attachment *is* lives below the engine's
//! seam - the `assets/` prefix rule, the extension allowlist, the path rules,
//! the hash, the mime, the size ceiling and the maintenance state a write
//! marks. This module is the HTTP shape around it: who may call, what the
//! headers say, and which status a refusal takes.
//!
//! **The one rule this layer owns is the response headers on a read.** A served
//! attachment is the only place this API hands a browser bytes somebody
//! uploaded, so every answer carries `X-Content-Type-Options: nosniff` (the
//! declared mime is the only one, never one a sniffer guesses from the bytes)
//! and `Content-Security-Policy: default-src 'none'; sandbox` (an SVG or an
//! HTML-shaped payload gets a unique opaque origin and can load nothing, so it
//! cannot reach the session that fetched it). `Content-Disposition` then follows
//! the mime alone: images, PDFs and text render in place because that is the
//! point of attaching them, and everything else - the office formats - arrives
//! as a download named after the file.
//!
//! Two statuses are worth naming, because the difference is what a client
//! branches on. A path the rules refuse is **400** on every verb, with the rule
//! that refused it in the detail: the caller can fix the request. A path that is
//! well formed and holds nothing is **404**: the caller cannot. The engine
//! reports the first as `Invalid` and the second as `NotFound`, and the only
//! translation this module makes is bending `Invalid` down from the generic 422
//! that [`crate::rest::ApiError`]'s classification would otherwise give it -
//! a malformed path is a malformed request, not an unprocessable one.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use crystalline_core::attachment::is_inline_attachment_mime;
use crystalline_index::AttachmentRow;

use super::auth::Identity;
use super::{ApiError, ApiPath, ProblemDetail, RestState, refuse_read_only};
use crate::engine::EngineError;

/// The `Content-Security-Policy` every attachment is served under: no origin
/// may be reached for anything, and the response is sandboxed into a unique
/// opaque origin so a scriptable payload cannot touch the instance it came
/// from.
const ATTACHMENT_CSP: &str = "default-src 'none'; sandbox";

/// The `Cache-Control` every attachment read carries: store it, but come back
/// and ask before using it.
///
/// Caching an attachment is worth having - the bytes are immutable for as long
/// as the path holds them, and the strong `ETag` makes the revalidation one
/// cheap 304 - but this response carries no freshness information of its own,
/// no `Expires` and no `Last-Modified`. RFC 9111 section 4.2.2 lets a cache
/// invent a lifetime for exactly that response and reuse it with **no request
/// to this server at all**, which would skip `If-None-Match` rather than skip
/// the body. Since a PUT to the same path is a legitimate replace-in-place,
/// that is a page rendering last week's diagram with no way to notice. So the
/// directive is `no-cache`, which despite its name means "store it, revalidate
/// it every time" rather than "do not store it": the round trip stays, the body
/// does not.
const REVALIDATE: &str = "no-cache";

/// One attachment's metadata, the row a file browser draws.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "One attachment a domain carries: where it lives, what \
                        it is, how big it is and when it last changed.")]
pub struct AttachmentView {
    /// The domain-relative path, always under `assets/`.
    #[schema(example = "assets/architecture.png")]
    pub path: String,
    /// The mime the bytes are served under, derived from the extension.
    #[schema(example = "image/png")]
    pub mime: String,
    /// Byte length.
    #[schema(example = 20481)]
    pub size: u64,
    /// Last modification instant, RFC 3339.
    #[schema(example = "2026-08-18T09:12:00+00:00")]
    pub modified: String,
    /// Lowercase hex SHA-256 of the bytes, the same token the read's `ETag`
    /// carries.
    #[schema(example = "9f2a1c05e2b7")]
    pub sha256: String,
}

impl From<AttachmentRow> for AttachmentView {
    fn from(row: AttachmentRow) -> AttachmentView {
        AttachmentView {
            path: row.path,
            mime: row.mime,
            size: row.size,
            modified: row.modified,
            sha256: row.sha256,
        }
    }
}

/// What the listing answers: every attachment of one domain, ordered by path.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(description = "Every attachment one domain carries, ordered by path. \
                        Metadata only: the bytes are fetched one file at a \
                        time.")]
pub struct AttachmentsResponse {
    /// The rows, ordered by path.
    pub attachments: Vec<AttachmentView>,
}

/// What an upload answers: enough for a client to write the markdown reference
/// and to cache what it just sent.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
#[schema(
    description = "The attachment as stored: the path to reference it by, \
                        the mime it will be served under, its size and the \
                        checksum a read's `ETag` will carry."
)]
pub struct UploadedAttachment {
    /// The domain-relative path it was stored at, the target an engram body
    /// references.
    #[schema(example = "assets/architecture.png")]
    pub path: String,
    /// The mime the bytes will be served under.
    #[schema(example = "image/png")]
    pub mime: String,
    /// Byte length as stored.
    #[schema(example = 20481)]
    pub size: u64,
    /// Lowercase hex SHA-256 of the stored bytes.
    #[schema(example = "9f2a1c05e2b7")]
    pub sha256: String,
}

/// `GET /domains/{domain}/files/{*path}` - one attachment's bytes.
///
/// Read auth like every other read here, and served on a read-only instance:
/// serving what it already holds is what a mirror is for.
///
/// The answer carries a strong `ETag` - the quoted SHA-256 of exactly the bytes
/// being sent - so a client that offers it back in `If-None-Match` is answered
/// 304 with no body. The hash is the file's own content, not a timestamp, so an
/// attachment that was rewritten with identical bytes is correctly reported
/// unchanged.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/files/{path}",
    tag = "attachments",
    operation_id = "read_attachment",
    summary = "One attachment's bytes.",
    description = "Serves the stored bytes under the mime the extension \
                   allowlist assigns - never one guessed from the content and \
                   never one the uploader claimed. Every answer carries \
                   `X-Content-Type-Options: nosniff` and a \
                   `default-src 'none'; sandbox` content security policy, so an \
                   attachment can never script against the instance serving \
                   it. Images, PDFs and text are dispositioned `inline`; the \
                   office formats arrive as a download.\n\nThe `ETag` is the \
                   strong quoted SHA-256 of the bytes, so `If-None-Match` \
                   answers 304 without a body, and `Cache-Control: no-cache` \
                   keeps a stored copy revalidating instead of going \
                   heuristically fresh, so a file replaced at the same path is \
                   picked up on its next use. A malformed path is 400 with the \
                   rule that refused it; a well-formed path holding nothing is \
                   404.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "path" = String,
            Path,
            description = "The domain-relative attachment path. It always \
                           starts with `assets/` and may contain slashes: \
                           `assets/diagrams/flow.png`.",
            example = "assets/diagrams/flow.png",
        ),
    ),
    responses(
        (
            status = 200,
            description = "The bytes, under the mime the allowlist assigns.",
            // Raw bytes. Declared as a string because utoipa's schema for
            // `Vec<u8>` is an array of integers, which would describe this
            // response as JSON; the content type carries the meaning.
            body = String,
            content_type = "application/octet-stream",
            headers(
                ("etag" = String, description = "The strong quoted SHA-256 of the bytes served."),
                ("cache-control" = String, description = "Always `no-cache`: store it, but revalidate before every use."),
                ("x-content-type-options" = String, description = "Always `nosniff`."),
                ("content-security-policy" = String, description = "Always `default-src 'none'; sandbox`."),
                ("content-disposition" = String, description = "`inline` for images, PDFs and text; `attachment; filename=\"...\"` otherwise."),
            ),
        ),
        (
            status = 304,
            description = "The `If-None-Match` token matches the stored bytes; \
                           no body is sent. Carries the `ETag` it matched and \
                           the same `Cache-Control`, so the stored response it \
                           refreshes keeps having to revalidate.",
        ),
        (
            status = 400,
            description = "The path breaks an attachment path rule: not under \
                           `assets/`, a `.` or `..` segment, a hidden segment, \
                           a refused character, too long, or an extension that \
                           is not on the allowlist.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or no attachment at that path.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn read(
    State(state): State<RestState>,
    headers: HeaderMap,
    ApiPath((domain, path)): ApiPath<(String, String)>,
) -> Result<Response, ApiError> {
    let (bytes, row) = state
        .engine
        .attachment_read(&domain, &path)
        .await
        .map_err(malformed_path_is_a_bad_request)?;
    let etag = format!("\"{}\"", row.sha256);
    if if_none_match_matches(&headers, &row.sha256) {
        // RFC 9110: a 304 carries the validator it matched and no body, so the
        // client can go on caching under the same token. It repeats
        // `Cache-Control` too, since a 304 updates the stored response's
        // headers and dropping the directive here would let the very response
        // it refreshes turn heuristically fresh.
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, REVALIDATE.to_string()),
            ],
        )
            .into_response());
    }
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, row.mime.clone()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, REVALIDATE.to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CONTENT_SECURITY_POLICY, ATTACHMENT_CSP.to_string()),
            (header::CONTENT_DISPOSITION, disposition(&row)),
        ],
        bytes,
    )
        .into_response())
}

/// `PUT /domains/{domain}/files/{*path}` - store an attachment, creating it or
/// replacing what is there.
///
/// The body is the raw bytes: no multipart, so a client sends the file itself
/// and the path it should live at rides the URL rather than a form field. The
/// declared content type is ignored on purpose - the extension allowlist
/// decides what this file is, at upload and at every later read, so the two can
/// never disagree.
///
/// Gate order: editor, the CSRF token the shared guard checks, the read-only
/// refusal, then the engine's own path, allowlist and size rules. A body over
/// the surface's 10 MiB limit is refused 413 by the body limit before this
/// handler runs at all.
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/files/{path}",
    tag = "attachments",
    operation_id = "write_attachment",
    summary = "Upload an attachment, creating or replacing it.",
    description = "Editor only. The request body is the raw file - not \
                   multipart - and the declared content type is ignored: the \
                   extension allowlist decides the mime, at upload and at every \
                   later read.\n\nThe path must start with `assets/`, hold no \
                   `.`, `..` or hidden segment, no backslash, colon or `#`, be \
                   at most 256 bytes and end in an allowlisted extension; \
                   anything else is 400 naming the rule. The domain is marked \
                   as owing a consolidation sweep, because a person just added \
                   something the agent has not read yet.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "path" = String,
            Path,
            description = "Where to store it, domain-relative and under \
                           `assets/`.",
            example = "assets/diagrams/flow.png",
        ),
    ),
    request_body(
        description = "The raw file bytes.",
        content_type = "application/octet-stream",
        content = String,
    ),
    responses(
        (
            status = 200,
            description = "Stored. The path to reference it by, its mime, its \
                           size and its checksum.",
            body = UploadedAttachment,
        ),
        (
            status = 400,
            description = "The path breaks an attachment path rule or carries \
                           an extension that is not on the allowlist.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one: an anonymous \
                           identity never writes.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an editor, the CSRF token is \
                           missing or wrong, or the instance is read-only.",
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
            description = "The body is over the 10 MiB limit this API accepts, \
                           which is also the attachment size ceiling.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn write(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, path)): ApiPath<(String, String)>,
    // Last, and the only extractor here that consumes the body.
    body: Bytes,
) -> Result<Json<UploadedAttachment>, ApiError> {
    identity.require_editor()?;
    refuse_read_only(&state)?;
    let row = state
        .engine
        .attachment_write(&domain, &path, body.to_vec())
        .await
        .map_err(malformed_path_is_a_bad_request)?;
    Ok(Json(UploadedAttachment {
        path: row.path,
        mime: row.mime,
        size: row.size,
        sha256: row.sha256,
    }))
}

/// `DELETE /domains/{domain}/files/{*path}` - remove an attachment.
///
/// The bytes and the metadata row go together, whichever of the two a
/// hand-edited domain had left. Nothing else is touched: an engram that still
/// references the path keeps its reference, and the consolidation sweep is what
/// reports the now-dangling link.
#[utoipa::path(
    delete,
    path = "/api/v1/domains/{domain}/files/{path}",
    tag = "attachments",
    operation_id = "delete_attachment",
    summary = "Delete an attachment.",
    description = "Editor only. Removes the bytes and the metadata row \
                   together. An engram that still references the path keeps its \
                   reference - the consolidation sweep is what reports the \
                   dangling link, rather than this route rewriting somebody's \
                   markdown.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        ("path" = String, Path, description = "The attachment path.", example = "assets/diagrams/flow.png"),
    ),
    responses(
        (status = 204, description = "Deleted."),
        (
            status = 400,
            description = "The path breaks an attachment path rule.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an editor, the CSRF token is \
                           missing or wrong, or the instance is read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or no attachment at that path.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn remove(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath((domain, path)): ApiPath<(String, String)>,
) -> Result<StatusCode, ApiError> {
    identity.require_editor()?;
    refuse_read_only(&state)?;
    state
        .engine
        .attachment_delete(&domain, &path)
        .await
        .map_err(malformed_path_is_a_bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /domains/{domain}/attachments` - every attachment the domain carries.
///
/// Metadata only, ordered by path: a domain full of slide decks costs one query
/// rather than a download.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/attachments",
    tag = "attachments",
    operation_id = "list_attachments",
    summary = "Every attachment a domain carries.",
    description = "Metadata only, ordered by path: no bytes are read, so \
                   listing a domain full of slide decks costs one query. Each \
                   row carries the path to fetch it by, its mime, its size, when \
                   it last changed and its checksum.",
    params(("domain" = String, Path, description = "The registered domain.")),
    responses(
        (status = 200, description = "The rows, ordered by path.", body = AttachmentsResponse),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
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
pub async fn list(
    State(state): State<RestState>,
    ApiPath(domain): ApiPath<String>,
) -> Result<Json<AttachmentsResponse>, ApiError> {
    let rows = state.engine.attachment_list(&domain).await?;
    Ok(Json(AttachmentsResponse {
        attachments: rows.into_iter().map(AttachmentView::from).collect(),
    }))
}

/// A path the attachment rules refuse is a 400 rather than the 422 the generic
/// classification gives an `Invalid`.
///
/// The distinction this preserves is the one a client acts on: 400 means the
/// request itself is malformed and the caller can correct it, 404 means the
/// address is fine and nothing is there. Every other variant keeps its usual
/// status.
fn malformed_path_is_a_bad_request(e: EngineError) -> ApiError {
    match e {
        EngineError::Invalid(detail) => ApiError::bad_request(detail),
        other => other.into(),
    }
}

/// Whether the request's `If-None-Match` covers the stored checksum.
///
/// Deliberately more forgiving than [`super::if_match`], which guards a write:
/// this one only decides whether to send bytes a client already has, so a list
/// of candidates, a `*` wildcard and a weak validator are all honoured rather
/// than refused. Nothing is lost by being wrong in the permissive direction
/// either, since the worst outcome is a full response the client discards.
fn if_none_match_matches(headers: &HeaderMap, sha256: &str) -> bool {
    let Some(raw) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*"
            || candidate
                .strip_prefix("W/")
                .unwrap_or(candidate)
                .trim_matches('"')
                == sha256
    })
}

/// The `Content-Disposition` for one attachment: `inline` when a browser should
/// render it where it stands, a named download otherwise.
fn disposition(row: &AttachmentRow) -> String {
    if is_inline_attachment_mime(&row.mime) {
        return "inline".to_string();
    }
    format!("attachment; filename=\"{}\"", download_filename(&row.path))
}

/// The suggested filename for a download: the path's final segment, defanged to
/// filename-safe ASCII.
///
/// The same treatment `archive::archive_filename` gives a domain name, and for
/// the same reason: nothing a stored path can carry may put a quote, a
/// separator or a header break into the header this builds. Attachment paths are
/// already validated against backslashes, colons, `#` and control characters,
/// but not against quotes or non-ASCII, and a header value is the wrong place to
/// discover that.
fn download_filename(path: &str) -> String {
    let last = path.rsplit('/').next().unwrap_or(path);
    let safe: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "attachment".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, mime: &str) -> AttachmentRow {
        AttachmentRow {
            path: path.to_string(),
            sha256: "abc123".to_string(),
            mime: mime.to_string(),
            size: 3,
            modified: "2026-08-18T09:12:00+00:00".to_string(),
        }
    }

    #[test]
    fn the_disposition_follows_the_mime_and_names_the_file() {
        assert_eq!(disposition(&row("assets/a.png", "image/png")), "inline");
        assert_eq!(
            disposition(&row("assets/a.pdf", "application/pdf")),
            "inline"
        );
        assert_eq!(
            disposition(&row("assets/a.json", "application/json")),
            "inline"
        );
        assert_eq!(
            disposition(&row(
                "assets/talks/deck.pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            )),
            "attachment; filename=\"deck.pptx\"",
            "a download is named after the file, not after the whole path"
        );
    }

    #[test]
    fn a_download_filename_can_never_break_the_header() {
        // A quote is the one character the path rules allow that would end the
        // quoted string early.
        assert_eq!(download_filename("assets/o\"ops.pptx"), "o-ops.pptx");
        assert_eq!(download_filename("assets/my deck.pptx"), "my-deck.pptx");
        assert_eq!(download_filename("assets/schöne.pptx"), "sch-ne.pptx");
        assert_eq!(download_filename("assets/"), "attachment");
    }

    #[test]
    fn if_none_match_honours_the_forms_a_cache_sends() {
        let with = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, value.parse().unwrap());
            if_none_match_matches(&headers, "abc123")
        };
        assert!(with("\"abc123\""));
        assert!(with("*"), "a wildcard asks about any version at all");
        assert!(with("W/\"abc123\""), "a weak validator still identifies it");
        assert!(with("\"other\", \"abc123\""), "one of a list is enough");
        assert!(!with("\"other\""));
        assert!(!with(""));
        assert!(
            !if_none_match_matches(&HeaderMap::new(), "abc123"),
            "no header asks for the bytes"
        );
    }
}
