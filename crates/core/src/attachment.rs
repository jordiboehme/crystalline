//! Attachments: the reserved `assets/` prefix, the extension allowlist, asset
//! path validation and the body reference scanner.
//!
//! An attachment is a file an Engram carries alongside its markdown - a
//! screenshot, a diagram, a slide deck, a data file. Attachments live under the
//! reserved `assets/` prefix at the domain root and are addressed as
//! `crystalline://<domain>/assets/<rel-path>`. Nothing under `assets/` is ever
//! parsed as an Engram.
//!
//! This module is the single source of truth for what may be attached: the
//! extension-to-mime table, the size cap and the path rules. It is pure, so the
//! service engine, the REST layer, the archive and the MCP surface all decide
//! the same way without duplicating a list.

/// The reserved path prefix every attachment lives under, at the domain root.
pub const ASSETS_PREFIX: &str = "assets/";

/// The largest attachment Crystalline stores, 10 MiB. Equal to the REST and
/// MCP body ceiling, so an upload that passes the transport passes here too.
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;

/// The longest asset path, in bytes, including the `assets/` prefix.
const MAX_ASSET_PATH_BYTES: usize = 256;

/// The mime type Crystalline serves a file under, derived from its extension,
/// or `None` when the extension is not on the allowlist.
///
/// Matching is case-insensitive, so `Deck.PPTX` and `deck.pptx` agree. A
/// client-supplied content type is never trusted; this table decides. `md` is
/// deliberately absent: markdown is an Engram, never an attachment.
pub fn attachment_mime(filename: &str) -> Option<&'static str> {
    let (_, extension) = filename.rsplit_once('.')?;
    let extension = extension.to_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "odt" => "application/vnd.oasis.opendocument.text",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "xml" => "application/xml",
        _ => return None,
    };
    Some(mime)
}

/// Whether a mime is read and served as text rather than as bytes: every
/// `text/*` plus the structured text formats JSON, YAML, TOML and XML.
pub fn is_text_attachment_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/yaml" | "application/toml" | "application/xml"
        )
}

/// Whether a browser renders the mime in place rather than downloading it:
/// images, PDFs and every text mime. Everything else - the office formats -
/// is served as a download.
pub fn is_inline_attachment_mime(mime: &str) -> bool {
    mime.starts_with("image/") || mime == "application/pdf" || is_text_attachment_mime(mime)
}

/// Why an asset path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssetPathError {
    /// The path does not start with the reserved `assets/` prefix.
    #[error("an attachment path must start with `assets/`")]
    NotUnderAssets,
    /// The path holds an empty segment (a doubled or trailing slash).
    #[error("an attachment path must not hold an empty segment")]
    EmptySegment,
    /// The path holds a `.` or `..` segment.
    #[error("an attachment path must not hold a `.` or `..` segment")]
    DotSegment,
    /// A segment starts with `.`, which would hide the file from sync.
    #[error("an attachment path must not hold a segment starting with `.`")]
    HiddenSegment,
    /// The path holds a backslash or a control character.
    #[error("an attachment path must not hold a backslash or a control character")]
    BadCharacter,
    /// The path is longer than 256 bytes.
    #[error("an attachment path must be at most 256 bytes")]
    TooLong,
    /// The final segment carries no allowlisted extension.
    #[error("an attachment must carry an allowlisted file extension")]
    DisallowedExtension,
}

/// Check a domain-relative attachment path against every rule.
///
/// Asset paths are never slugified - a filename a human recognizes is the
/// point - so validation carries the whole burden: the path starts with
/// `assets/`, uses forward slashes only, holds no empty, `.`, `..` or
/// dot-leading segment, holds no backslash or control character, is at most
/// 256 bytes, and its final segment carries an allowlisted extension.
pub fn validate_asset_path(path: &str) -> Result<(), AssetPathError> {
    let Some(rest) = path.strip_prefix(ASSETS_PREFIX) else {
        return Err(AssetPathError::NotUnderAssets);
    };
    if path.len() > MAX_ASSET_PATH_BYTES {
        return Err(AssetPathError::TooLong);
    }
    if path.contains('\\') || path.chars().any(char::is_control) {
        return Err(AssetPathError::BadCharacter);
    }

    let mut last = "";
    for segment in rest.split('/') {
        if segment.is_empty() {
            return Err(AssetPathError::EmptySegment);
        }
        if segment == "." || segment == ".." {
            return Err(AssetPathError::DotSegment);
        }
        if segment.starts_with('.') {
            return Err(AssetPathError::HiddenSegment);
        }
        last = segment;
    }
    if attachment_mime(last).is_none() {
        return Err(AssetPathError::DisallowedExtension);
    }
    Ok(())
}

/// The distinct `assets/` targets an Engram body references, in order of first
/// appearance.
///
/// Both markdown forms count: `![alt](assets/shot.png)` images and
/// `[text](assets/deck.pdf)` links. A leading `./` is stripped, a title clause
/// after the target (`(assets/x.png "Q3")`) is dropped, and fenced code blocks
/// are skipped so an example in a snippet never counts as a reference. A target
/// carrying a scheme or a leading `/` addresses something outside the domain
/// and is ignored, which the `assets/` prefix test already decides.
pub fn find_asset_refs(body: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        for target in line_targets(line) {
            if !refs.contains(&target) {
                refs.push(target);
            }
        }
    }
    refs
}

/// The `assets/` link targets on one line, in order, duplicates included.
fn line_targets(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut targets = Vec::new();
    let mut idx = 0;
    while let Some(hit) = line[idx..].find("](") {
        let open = idx + hit + 2;
        // Markdown allows balanced parentheses inside a destination, so the
        // closing one is the depth-zero `)`, not the first.
        let mut depth = 1usize;
        let mut end = None;
        for (offset, byte) in bytes[open..].iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        idx = end + 1;
        let inside = line[open..end].trim();
        let target = inside.split_whitespace().next().unwrap_or("");
        let target = target.strip_prefix("./").unwrap_or(target);
        if target.starts_with(ASSETS_PREFIX) {
            targets.push(target.to_string());
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every extension the specification allowlists, with the exact mime the
    /// server must serve it under.
    const SPEC_TABLE: &[(&str, &str)] = &[
        ("png", "image/png"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("gif", "image/gif"),
        ("webp", "image/webp"),
        ("svg", "image/svg+xml"),
        ("pdf", "application/pdf"),
        (
            "pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
        ("odp", "application/vnd.oasis.opendocument.presentation"),
        (
            "docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        ("odt", "application/vnd.oasis.opendocument.text"),
        (
            "xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
        ("txt", "text/plain"),
        ("csv", "text/csv"),
        ("json", "application/json"),
        ("log", "text/plain"),
        ("yaml", "application/yaml"),
        ("yml", "application/yaml"),
        ("toml", "application/toml"),
        ("xml", "application/xml"),
    ];

    #[test]
    fn every_allowlisted_extension_maps_to_its_mime() {
        for (extension, mime) in SPEC_TABLE {
            assert_eq!(
                attachment_mime(&format!("file.{extension}")),
                Some(*mime),
                "extension {extension}"
            );
        }
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(
            attachment_mime("Deck.PPTX"),
            Some("application/vnd.openxmlformats-officedocument.presentationml.presentation")
        );
        assert_eq!(attachment_mime("Shot.PNG"), Some("image/png"));
    }

    #[test]
    fn markdown_and_unlisted_extensions_are_not_attachments() {
        assert_eq!(attachment_mime("x.md"), None);
        assert_eq!(attachment_mime("x.exe"), None);
        assert_eq!(attachment_mime("noextension"), None);
    }

    #[test]
    fn text_mimes_are_the_structured_ones_plus_every_text_type() {
        assert!(is_text_attachment_mime("text/plain"));
        assert!(is_text_attachment_mime("text/csv"));
        assert!(is_text_attachment_mime("application/json"));
        assert!(is_text_attachment_mime("application/yaml"));
        assert!(is_text_attachment_mime("application/toml"));
        assert!(is_text_attachment_mime("application/xml"));
        assert!(!is_text_attachment_mime("image/png"));
        assert!(!is_text_attachment_mime("application/pdf"));
    }

    #[test]
    fn inline_mimes_are_images_pdfs_and_text_and_office_formats_download() {
        assert!(is_inline_attachment_mime("image/png"));
        assert!(is_inline_attachment_mime("image/svg+xml"));
        assert!(is_inline_attachment_mime("application/pdf"));
        assert!(is_inline_attachment_mime("text/plain"));
        assert!(is_inline_attachment_mime("application/json"));
        assert!(!is_inline_attachment_mime(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        ));
        assert!(!is_inline_attachment_mime(
            "application/vnd.oasis.opendocument.text"
        ));
    }

    #[test]
    fn a_nested_allowlisted_path_is_accepted() {
        assert_eq!(validate_asset_path("assets/a/b.png"), Ok(()));
        assert_eq!(validate_asset_path("assets/deck.pdf"), Ok(()));
    }

    #[test]
    fn each_refused_path_names_its_reason() {
        assert_eq!(
            validate_asset_path("notes/a.png"),
            Err(AssetPathError::NotUnderAssets)
        );
        assert_eq!(
            validate_asset_path("assets/../x.png"),
            Err(AssetPathError::DotSegment)
        );
        assert_eq!(
            validate_asset_path("assets/.hidden/x.png"),
            Err(AssetPathError::HiddenSegment)
        );
        assert_eq!(
            validate_asset_path("assets//x.png"),
            Err(AssetPathError::EmptySegment)
        );
        assert_eq!(
            validate_asset_path("assets/x.exe"),
            Err(AssetPathError::DisallowedExtension)
        );
        assert_eq!(
            validate_asset_path("assets/a\\b.png"),
            Err(AssetPathError::BadCharacter)
        );
        let long = format!("assets/{}.png", "a".repeat(300));
        assert!(long.len() > 300);
        assert_eq!(validate_asset_path(&long), Err(AssetPathError::TooLong));
    }

    #[test]
    fn the_scanner_returns_distinct_relative_targets_in_body_order() {
        let body = "\
![d](assets/flow.png)\n\
\n\
[deck](./assets/deck.pdf \"Q3\")\n\
\n\
Again: ![d](assets/flow.png)\n\
\n\
[ext](https://e/x.png) and [abs](/assets/x.png)\n\
\n\
```\n\
![f](assets/fenced.png)\n\
```\n";
        assert_eq!(
            find_asset_refs(body),
            vec!["assets/flow.png".to_string(), "assets/deck.pdf".to_string()]
        );
    }

    #[test]
    fn a_body_with_no_references_yields_nothing() {
        assert_eq!(
            find_asset_refs("plain prose, no links"),
            Vec::<String>::new()
        );
    }
}
