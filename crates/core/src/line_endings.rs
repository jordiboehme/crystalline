//! The one line ending Crystalline stores and returns: `\n`.
//!
//! Every file Crystalline writes into a domain is UTF-8 with LF line endings,
//! and everything it returns (a read, a search snippet, an index row) is LF
//! too. CRLF is still accepted wherever text comes in - a file on disk, a tool
//! parameter - and [`to_lf`] is the single conversion every such boundary
//! calls. A file Crystalline only reads is never rewritten: a CRLF file turns
//! into LF on the next write that lands on it, and then as a whole.
//!
//! **One exception: a share from a domain that reviews changes keeps the
//! team's line endings.** A draft is stored as LF like everything else, but
//! its share is staged with [`with_line_endings_of`] against the team's own
//! copy, so a wholly CRLF team file comes back CRLF and the proposal or commit
//! shows only the lines she changed, not every line. Everything else stays LF:
//! a share of a domain that takes changes directly, and a fold of the drafts
//! into the folder, land LF as above.

use std::borrow::Cow;

/// `text` with every line ending as a plain `\n`.
///
/// Each `\r` that sits directly before a `\n` goes, a run of them included, so
/// `"a\r\r\nb"` becomes `"a\nb"` and the result never holds a `\r\n` pair: the
/// conversion is idempotent. A lone `\r` with no `\n` behind it is line
/// content, not a line ending, and stays - which also keeps every line number
/// the same before and after, since no line is split or joined.
///
/// Borrows when there is nothing to convert, which is the common case.
pub fn to_lf(text: &str) -> Cow<'_, str> {
    if !text.contains("\r\n") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut pending_cr = 0usize;
    for c in text.chars() {
        match c {
            '\r' => pending_cr += 1,
            '\n' => {
                pending_cr = 0;
                out.push('\n');
            }
            other => {
                for _ in 0..pending_cr {
                    out.push('\r');
                }
                pending_cr = 0;
                out.push(other);
            }
        }
    }
    for _ in 0..pending_cr {
        out.push('\r');
    }
    Cow::Owned(out)
}

/// `text` in the line endings of `team`: CRLF when every line ending of `team`
/// is CRLF and it has at least one, otherwise `text` as it is.
///
/// The one place a text Crystalline writes out is not LF (see the module doc).
/// `text` goes through [`to_lf`] first, so a mixed text cannot come out with
/// some lines doubled, and `to_lf` of the result is always `to_lf(text)`. A
/// team file with mixed endings, or with no line ending at all, says nothing
/// clear, so the text keeps the stored form.
pub fn with_line_endings_of<'t>(text: &'t str, team: &[u8]) -> Cow<'t, str> {
    let lines = team.iter().filter(|&&byte| byte == b'\n').count();
    let crlf = team.windows(2).filter(|pair| pair == b"\r\n").count();
    if lines == 0 || lines != crlf {
        return Cow::Borrowed(text);
    }
    Cow::Owned(to_lf(text).replace('\n', "\r\n"))
}

/// [`to_lf`] over raw bytes, refusing anything that is not UTF-8.
///
/// The error is the one-line message a write path hands back when it is given
/// bytes that cannot be an engram, MANIFEST or any other domain text file.
pub fn utf8_to_lf(bytes: &[u8]) -> Result<String, NotUtf8> {
    let text = std::str::from_utf8(bytes).map_err(|e| NotUtf8 {
        offset: e.valid_up_to(),
    })?;
    Ok(to_lf(text).into_owned())
}

/// Text that is not valid UTF-8, with the byte offset of the first bad
/// sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the text is not valid UTF-8 (first bad byte at offset {offset}); save it as UTF-8")]
pub struct NotUtf8 {
    /// The byte offset of the first invalid sequence.
    pub offset: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lf_text_is_borrowed_unchanged() {
        for text in ["", "a\nb\n", "a lone \r stays", "no newline"] {
            assert!(
                matches!(to_lf(text), Cow::Borrowed(t) if t == text),
                "{text:?}"
            );
        }
    }

    #[test]
    fn crlf_and_mixed_text_become_lf() {
        assert_eq!(to_lf("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(to_lf("a\r\nmixed\nline\r\n"), "a\nmixed\nline\n");
        assert_eq!(
            to_lf("---\r\ntitle: A\r\n---\r\n\r\nbody"),
            "---\ntitle: A\n---\n\nbody"
        );
    }

    #[test]
    fn a_run_of_cr_before_a_newline_goes_whole() {
        assert_eq!(to_lf("a\r\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn a_lone_cr_is_content_and_stays() {
        assert_eq!(to_lf("a\rb\r\nc\r"), "a\rb\nc\r");
        assert_eq!(to_lf("x\r\r y\r\n"), "x\r\r y\n");
    }

    #[test]
    fn the_result_never_holds_crlf_and_is_idempotent() {
        for text in [
            "a\r\r\nb",
            "\r\n\r\n",
            "\r\r\r\n",
            "x\r\ny\rz\r\n\r",
            "日本\r\n語\r\n",
        ] {
            let once = to_lf(text).into_owned();
            assert!(!once.contains("\r\n"), "{text:?} -> {once:?}");
            assert_eq!(to_lf(&once), once.as_str(), "idempotent for {text:?}");
            assert_eq!(
                once.matches('\n').count(),
                text.matches('\n').count(),
                "line count kept for {text:?}"
            );
        }
    }

    #[test]
    fn a_wholly_crlf_team_file_gives_its_endings() {
        let out = with_line_endings_of("a\nb\n", b"x\r\ny\r\n");
        assert_eq!(out, "a\r\nb\r\n");
        // A text already in CRLF, or mixed, is not doubled.
        assert_eq!(with_line_endings_of("a\r\nb\nc", b"x\r\n"), "a\r\nb\r\nc");
    }

    #[test]
    fn any_other_team_file_leaves_the_text_as_it_is() {
        for team in [
            &b"x\ny\n"[..],
            b"x\r\ny\n",
            b"no line ending",
            b"",
            b"a lone \r only",
        ] {
            assert!(
                matches!(
                    with_line_endings_of("a\nb\n", team),
                    Cow::Borrowed("a\nb\n")
                ),
                "{team:?}"
            );
        }
    }

    #[test]
    fn the_team_endings_round_trip_to_the_stored_text() {
        for text in ["a\nb\n", "a\r\nb", "x\r\ny\rz\n", "", "\n\n"] {
            let out = with_line_endings_of(text, b"t\r\n");
            assert_eq!(to_lf(&out), to_lf(text), "{text:?}");
            assert!(wholly_crlf_or_empty(&out), "{text:?} -> {out:?}");
        }
    }

    fn wholly_crlf_or_empty(text: &str) -> bool {
        text.matches('\n').count() == text.matches("\r\n").count()
    }

    #[test]
    fn bytes_that_are_not_utf8_are_refused_with_one_line() {
        let err = utf8_to_lf(b"ok\r\n\xff\xfe").unwrap_err();
        assert_eq!(err.offset, 4);
        let message = err.to_string();
        assert!(!message.contains('\n'), "{message}");
        assert!(message.contains("not valid UTF-8"), "{message}");
        assert_eq!(utf8_to_lf(b"a\r\nb").unwrap(), "a\nb");
    }
}
