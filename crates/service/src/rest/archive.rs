//! Domain archives (admin): the zip download of a whole domain.
//!
//! Two decisions this module encodes. The archive is built in memory as one
//! `Vec<u8>` under `spawn_blocking` rather than streamed - a zip writer is
//! synchronous, and at the sizes this surface serves a buffer beats a
//! streaming writer's complexity. And the download is served on a read-only
//! instance: it mutates nothing, and a read-only mirror is exactly where an
//! operator wants a backup they can take.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::auth::Identity;
use super::{ApiError, ApiPath, ProblemDetail, RestState};

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
