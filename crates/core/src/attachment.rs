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

/// The reserved folder's own name, the [`ASSETS_PREFIX`] without its separator.
/// The two must always agree.
pub const ASSETS_FOLDER: &str = "assets";

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

/// Whether a forward-slashed, domain-relative path sits under the reserved
/// attachment folder, matching the folder name case-insensitively.
///
/// A stored attachment path has exactly one spelling - [`validate_asset_path`]
/// accepts only the lowercase `assets/` - but the *reservation* has to be wider
/// than that spelling: APFS and NTFS resolve `Assets` and `assets` to one
/// directory, so on the two filesystems most people run a folder that differs
/// only in case IS the reserved folder. Every classifier asks this one
/// question (the engine's engram write refusals, the sync walk, the daemon's
/// watcher), so the reservation gives one answer wherever it is asked.
///
/// Only the first segment is examined, so `assets-notes/x.md` and a root
/// `assets.md` are ordinary knowledge, as they should be.
pub fn is_under_assets(rel: &str) -> bool {
    rel.split('/')
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case(ASSETS_FOLDER))
}

/// The same path with its reserved folder segment folded to the canonical
/// `assets` spelling, or `None` when the path is not under the folder at all.
///
/// Everything after the first segment is left exactly as it was: only the
/// folder's own case is normalized, because that is the only part two spellings
/// of one directory can disagree about.
pub fn canonical_asset_path(rel: &str) -> Option<String> {
    if !is_under_assets(rel) {
        return None;
    }
    Some(match rel.split_once('/') {
        Some((_, rest)) => format!("{ASSETS_PREFIX}{rest}"),
        None => ASSETS_FOLDER.to_string(),
    })
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
    /// The path holds a backslash, a colon, a `#`, a `%`, a space, a
    /// parenthesis or a control character.
    #[error(
        "an attachment path must not hold a backslash, a colon, a `#`, a `%`, a space, a parenthesis or a control character"
    )]
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
/// dot-leading segment, holds no backslash, colon, `#`, `%`, space, parenthesis
/// or control character, is at most 256 bytes, and its final segment carries an
/// allowlisted extension.
///
/// A colon is refused because it is a path separator on some platforms and a
/// drive designator on Windows, so a segment shaped like `C:` would reach the
/// filesystem as something other than the plain relative name it looks like. A
/// `#` is refused because it opens the image formatting fragment a body target
/// may carry; keeping it out of stored paths is what makes that fragment
/// unambiguous.
///
/// A `%` is refused because it opens a percent escape, and a stored path is
/// read back through two layers that decode one: the MCP surface
/// percent-decodes an attachment identifier before it reaches this validator,
/// and the markdown renderer percent-encodes a link target on its way into
/// HTML. A literal `%` therefore makes one file answer to two spellings - the
/// stored `assets/100%.png` and the encoded `assets/100%25.png` - and the
/// scanner, the rail and the sweep would not have to agree about which one is
/// the reference. Keeping `%` out of stored paths is what keeps that question
/// from existing. The body scanner does decode a reference it finds
/// (`find_asset_refs`), which is what lets an encoded link resolve to the file
/// it renders; a stored path still never carries a literal `%`, so decoding a
/// reference can only ever land on one spelling.
///
/// A space and a parenthesis are refused because a markdown link target cannot
/// carry them reliably - a space ends the target and an unbalanced `)` closes
/// the link - so a file named that way risks being unreferenceable, and an
/// attachment no engram can reference must not exist.
pub fn validate_asset_path(path: &str) -> Result<(), AssetPathError> {
    let Some(rest) = path.strip_prefix(ASSETS_PREFIX) else {
        return Err(AssetPathError::NotUnderAssets);
    };
    if path.len() > MAX_ASSET_PATH_BYTES {
        return Err(AssetPathError::TooLong);
    }
    if path.contains('\\')
        || path.contains(':')
        || path.contains('#')
        || path.contains('%')
        || path.contains(' ')
        || path.contains('(')
        || path.contains(')')
        || path.chars().any(char::is_control)
    {
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
///
/// A target is percent-decoded before any of that is read off it, so
/// `assets/caf%C3%A9.png` claims `assets/café.png` and an encoded `%23` opens
/// a fragment; a malformed escape or a sequence that is not valid UTF-8 leaves
/// the target raw. That is the rule Fluid's scanner follows, and the two agree
/// so a reference the page renders is never reported as unreferenced.
///
/// A trailing `#fragment` is stripped: an image formatting directive
/// (`assets/pic.png#right,w=50%`) is a rendering instruction, not part of the
/// path, so it resolves and dedupes as `assets/pic.png`. Since a stored asset
/// path can never hold a `#`, the first one always opens the fragment. A target
/// that is nothing but the prefix once the fragment is gone (`assets/#left`)
/// names no file and is dropped.
pub fn find_asset_refs(body: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    // A fence opens on a marker of at least three backticks or tildes and
    // closes only on the same character repeated at least as many times, the
    // rule the rest of the crate reads fences by. A plain toggle would treat a
    // shorter marker inside a longer fence as a close, which both leaks a
    // fenced reference and drops the genuine ones after the true close.
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let text = line.trim_end_matches('\r');
        match fence {
            None => {
                if let Some((c, n, _)) = crate::parse::fence_marker(text) {
                    fence = Some((c, n));
                    continue;
                }
            }
            Some((fc, fcount)) => {
                if let Some((c, n, _)) = crate::parse::fence_marker(text)
                    && c == fc
                    && n >= fcount
                    && text.trim_start()[n..].trim().is_empty()
                {
                    fence = None;
                }
                continue;
            }
        }
        for target in line_targets(line) {
            if !refs.contains(&target) {
                refs.push(target);
            }
        }
    }
    refs
}

/// One hex digit of a percent escape, or `None` for anything else.
fn hex_nibble(byte: Option<&u8>) -> Option<u8> {
    let byte = *byte?;
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode a link target the way a browser's `decodeURIComponent`
/// does: `%XX` hex pairs become bytes, the result must be valid UTF-8, and
/// any malformed escape or invalid sequence leaves the target untouched -
/// the same catch-and-keep fallback Fluid's `decodeTarget` uses, so the two
/// scanners read one spelling.
fn percent_decode_target(target: &str) -> String {
    // The common case by far: nothing to decode, so the target allocates once
    // as a whole instead of being pushed back together byte by byte.
    if !target.contains('%') {
        return target.to_string();
    }
    let bytes = target.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // Both digits must be plain ASCII hex. A parse would also accept a
            // sign (`%+A`), which `decodeURIComponent` throws on, so the pair
            // is read nibble by nibble instead.
            let (Some(hi), Some(lo)) = (hex_nibble(bytes.get(i + 1)), hex_nibble(bytes.get(i + 2)))
            else {
                return target.to_string();
            };
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    match String::from_utf8(out) {
        Ok(decoded) => decoded,
        Err(_) => target.to_string(),
    }
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
        // The whole target decodes before anything is read off it, the order
        // Fluid's scanner uses: an encoded `%23` becomes a real `#` and then
        // opens the formatting fragment, an encoded prefix becomes `assets/`.
        let target = percent_decode_target(target);
        let target = target.strip_prefix("./").unwrap_or(&target);
        // An image formatting fragment is a rendering directive, not part of
        // the path, so it never reaches a reference. `#` cannot occur inside a
        // stored asset path, so the first one always opens the fragment.
        let target = match target.split_once('#') {
            Some((path, _)) => path,
            None => target,
        };
        if let Some(rest) = target.strip_prefix(ASSETS_PREFIX)
            && !rest.is_empty()
        {
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
    fn the_reserved_folder_is_recognized_whatever_its_case() {
        assert!(is_under_assets("assets/shot.png"));
        assert!(is_under_assets("Assets/shot.png"));
        assert!(is_under_assets("ASSETS/deep/deck.pdf"));
        assert!(is_under_assets("assets"));
        // Only the first segment, so a neighbour and a file are ordinary.
        assert!(!is_under_assets("assets-notes/x.md"));
        assert!(!is_under_assets("assets.md"));
        assert!(!is_under_assets("notes/assets/x.png"));
        assert!(!is_under_assets(""));
    }

    #[test]
    fn folding_a_path_touches_the_folder_segment_and_nothing_else() {
        assert_eq!(
            canonical_asset_path("Assets/Deep/Shot.PNG").as_deref(),
            Some("assets/Deep/Shot.PNG")
        );
        assert_eq!(
            canonical_asset_path("assets/shot.png").as_deref(),
            Some("assets/shot.png")
        );
        assert_eq!(canonical_asset_path("ASSETS").as_deref(), Some("assets"));
        assert_eq!(canonical_asset_path("notes/shot.png"), None);
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
    fn a_longer_fence_closes_only_on_a_marker_at_least_as_long() {
        // The inner three-backtick line is content, not a close, so the
        // reference beside it stays fenced; the reference after the true
        // four-backtick close is a genuine one and must be found.
        let body = "\
````\n\
```\n\
![y](assets/fenced.png)\n\
````\n\
\n\
![x](assets/real.png)\n";
        assert_eq!(find_asset_refs(body), vec!["assets/real.png".to_string()]);
    }

    #[test]
    fn an_image_format_fragment_is_stripped_and_dedupes_with_the_bare_path() {
        let body = "\
![x](assets/a.png#right,w=50%)\n\
\n\
![x again](assets/a.png)\n\
\n\
[deck](assets/deck.pdf#center)\n";
        assert_eq!(
            find_asset_refs(body),
            vec!["assets/a.png".to_string(), "assets/deck.pdf".to_string()]
        );
    }

    #[test]
    fn a_target_that_is_only_a_fragment_is_not_a_reference() {
        assert_eq!(find_asset_refs("![x](assets/#left)"), Vec::<String>::new());
        assert_eq!(find_asset_refs("![x](assets/#)"), Vec::<String>::new());
    }

    /// `decodeURIComponent` throws on an escape whose digits are not both plain
    /// hex, and on a `%` with fewer than two characters behind it, so both
    /// leave the target raw here too. A signed pair (`%+A`) is the one an
    /// integer parse would have accepted and the browser does not.
    #[test]
    fn an_escape_that_is_not_two_hex_digits_leaves_the_target_raw() {
        assert_eq!(
            find_asset_refs("![x](assets/bad%+Aname.png)"),
            vec!["assets/bad%+Aname.png".to_string()]
        );
        assert_eq!(
            find_asset_refs("![x](assets/trailing.png%4)"),
            vec!["assets/trailing.png%4".to_string()]
        );
    }

    #[test]
    fn a_colon_in_a_segment_is_refused() {
        assert_eq!(
            validate_asset_path("assets/C:/Users/x.png"),
            Err(AssetPathError::BadCharacter)
        );
        assert_eq!(
            validate_asset_path("assets/a:b.png"),
            Err(AssetPathError::BadCharacter)
        );
    }

    #[test]
    fn a_hash_in_a_path_is_refused() {
        assert_eq!(
            validate_asset_path("assets/a#b.png"),
            Err(AssetPathError::BadCharacter)
        );
    }

    /// A `%` is refused because two layers decode one on the way back: the MCP
    /// surface percent-decodes an identifier before it reaches this validator,
    /// and the markdown renderer percent-encodes a link target on its way into
    /// HTML. Both spellings below would otherwise name the same file, and no
    /// surface would be wrong to pick either.
    #[test]
    fn a_percent_in_a_path_is_refused() {
        assert_eq!(
            validate_asset_path("assets/100%.png"),
            Err(AssetPathError::BadCharacter)
        );
        assert_eq!(
            validate_asset_path("assets/100%25.png"),
            Err(AssetPathError::BadCharacter)
        );
        assert_eq!(
            validate_asset_path("assets/%2e%2e/x.png"),
            Err(AssetPathError::BadCharacter)
        );
    }

    /// A space and a parenthesis are refused because a markdown link target
    /// cannot carry them reliably, so a file named that way risks being
    /// unreferenceable - and the specification's invariant is that an
    /// attachment no engram can reference must not exist.
    ///
    /// The scanner assertions below are the evidence rather than decoration:
    /// they show what the reference to such a file actually resolves to, which
    /// in both cases is a different path that does not exist. The parenthesis
    /// class is refused whole rather than by half - [`find_asset_refs`] does
    /// track balanced pairs, so `shot(1).png` happens to survive - because a
    /// rule a person can hold ("no parentheses") beats one that depends on
    /// whether the other one is there.
    #[test]
    fn a_space_or_a_parenthesis_in_a_path_is_refused() {
        assert_eq!(
            validate_asset_path("assets/Q3 report.pdf"),
            Err(AssetPathError::BadCharacter)
        );
        assert_eq!(
            validate_asset_path("assets/shot(1).png"),
            Err(AssetPathError::BadCharacter)
        );
        assert_eq!(
            validate_asset_path("assets/deck).pdf"),
            Err(AssetPathError::BadCharacter)
        );

        // A space ends the target, so a reference to `assets/Q3 report.pdf`
        // resolves as `assets/Q3`; an unbalanced `)` closes the link, so a
        // reference to `assets/deck).pdf` resolves as `assets/deck`. Neither
        // names the file, and neither would validate.
        assert_eq!(
            find_asset_refs("[q](assets/Q3 report.pdf)"),
            vec!["assets/Q3".to_string()]
        );
        assert_eq!(
            find_asset_refs("[d](assets/deck).pdf)"),
            vec!["assets/deck".to_string()]
        );
        assert!(validate_asset_path("assets/Q3").is_err());
        assert!(validate_asset_path("assets/deck").is_err());
    }

    #[test]
    fn a_body_with_no_references_yields_nothing() {
        assert_eq!(
            find_asset_refs("plain prose, no links"),
            Vec::<String>::new()
        );
    }
}
