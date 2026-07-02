//! Small helpers shared across rule modules.

/// The one-based source line at which a file's body begins, matching the
/// numbering [`crate::parse::parse_engram`] uses for headings, observations,
/// relations and links.
pub(crate) fn body_line_start(source: &str) -> usize {
    let (_, _, body_start) = crate::parse::locate(source);
    source[..body_start].bytes().filter(|b| *b == b'\n').count() + 1
}
