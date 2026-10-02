//! A marked block Crystalline manages inside a text file the person owns
//! (`~/.gemini/GEMINI.md`, `~/.qwen/QWEN.md`, later the AGENTS.md family).
//!
//! The block is appended after one blank line when there is none, the text
//! between the markers is replaced when it differs, and removal takes out the
//! markers inclusive plus the blank line the append added, so the file comes
//! back byte for byte. A file with exactly one of the two markers, or with
//! more than one block, is refused and left alone.

use std::fmt;

/// The first line of the block (spec 3.4).
pub const BLOCK_BEGIN: &str = "<!-- BEGIN CRYSTALLINE (managed by crystalline install; edits inside this block are replaced) -->";
/// The last line of the block.
pub const BLOCK_END: &str = "<!-- END CRYSTALLINE -->";

/// Why a file's block was not touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// Only one of the two markers is there, or they are in the wrong order.
    OneMarker,
    /// More than one block.
    SeveralBlocks,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockError::OneMarker => write!(
                f,
                "the file has only one of the two Crystalline block markers. Remove the leftover marker line and run the command again"
            ),
            BlockError::SeveralBlocks => write!(
                f,
                "the file has more than one Crystalline block. Keep one and run the command again"
            ),
        }
    }
}

impl std::error::Error for BlockError {}

/// Where the block sits: byte offsets of the BEGIN line start, the end of
/// the BEGIN line (after its newline), the start of the END line and the
/// end of the END line (after its newline, if it has one).
struct Found {
    begin_start: usize,
    begin_end: usize,
    end_start: usize,
    end_end: usize,
}

fn find(text: &str) -> Result<Option<Found>, BlockError> {
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r').trim();
        if content == BLOCK_BEGIN {
            begins.push((at, at + line.len()));
        } else if content == BLOCK_END {
            ends.push((at, at + line.len()));
        }
        at += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([begin], [end]) if begin.1 <= end.0 => Ok(Some(Found {
            begin_start: begin.0,
            begin_end: begin.1,
            end_start: end.0,
            end_end: end.1,
        })),
        (b, e) if b.len() > 1 || e.len() > 1 => Err(BlockError::SeveralBlocks),
        _ => Err(BlockError::OneMarker),
    }
}

/// The newline the file uses: CRLF when it has one, else LF.
fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Write the block with `body` into `text` (`None`: the file does not exist).
/// Returns `None` when the block is already there with this body.
pub fn upsert_block(text: Option<&str>, body: &str) -> Result<Option<String>, BlockError> {
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        return Ok(Some(format!("{BLOCK_BEGIN}\n{body}\n{BLOCK_END}\n")));
    };
    let nl = newline(text);
    let body = body.replace("\r\n", "\n").replace('\n', nl);
    match find(text)? {
        Some(found) => {
            let inner = format!("{body}{nl}");
            if text[found.begin_end..found.end_start] == inner {
                return Ok(None);
            }
            let mut out = String::with_capacity(text.len() + inner.len());
            out.push_str(&text[..found.begin_end]);
            out.push_str(&inner);
            out.push_str(&text[found.end_start..]);
            Ok(Some(out))
        }
        None => {
            let block = format!("{BLOCK_BEGIN}{nl}{body}{nl}{BLOCK_END}");
            Ok(Some(if text.ends_with('\n') {
                format!("{text}{nl}{block}{nl}")
            } else {
                format!("{text}{nl}{nl}{block}")
            }))
        }
    }
}

/// Take the block out of `text`, with the blank line before it. Returns
/// `None` when there is no block.
pub fn remove_block(text: &str) -> Result<Option<String>, BlockError> {
    let Some(found) = find(text)? else {
        return Ok(None);
    };
    let nl = newline(text);
    let mut before = &text[..found.begin_start];
    let after = &text[found.end_end..];
    // The blank line the append put before the block.
    let blank = format!("{nl}{nl}");
    if before.ends_with(&blank) || before == nl {
        before = &before[..before.len() - nl.len()];
    }
    // A block appended to text without a final newline ends without one,
    // and the append added that newline too.
    let end_line = &text[found.end_start..found.end_end];
    if after.is_empty() && !end_line.ends_with('\n') && before.ends_with(nl) {
        before = &before[..before.len() - nl.len()];
    }
    Ok(Some(format!("{before}{after}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "Crystalline pointer";

    #[test]
    fn the_block_round_trips_every_ending() {
        for original in ["notes\n", "notes", "a\n\nb\n", "# Title\r\n\r\ntext\r\n"] {
            let with = upsert_block(Some(original), BODY)
                .unwrap()
                .expect("a change");
            assert!(with.contains(BLOCK_BEGIN) && with.contains(BLOCK_END));
            assert_eq!(
                upsert_block(Some(&with), BODY).unwrap(),
                None,
                "a second write is no change"
            );
            assert_eq!(
                remove_block(&with).unwrap().unwrap(),
                original,
                "{original:?}"
            );
        }
    }

    #[test]
    fn a_missing_file_gets_only_the_block_and_its_removal_leaves_nothing() {
        let with = upsert_block(None, BODY).unwrap().unwrap();
        assert_eq!(with, format!("{BLOCK_BEGIN}\n{BODY}\n{BLOCK_END}\n"));
        assert_eq!(remove_block(&with).unwrap().unwrap(), "");
    }

    #[test]
    fn text_inside_is_replaced_and_text_around_is_kept() {
        let with = upsert_block(Some("before\n"), BODY).unwrap().unwrap();
        let edited = with.replace(BODY, "somebody edited this") + "after\n";
        let rewritten = upsert_block(Some(&edited), BODY).unwrap().unwrap();
        assert!(rewritten.contains(BODY) && rewritten.ends_with("after\n"));
        assert_eq!(
            remove_block(&rewritten).unwrap().unwrap(),
            "before\nafter\n"
        );
    }

    #[test]
    fn a_broken_marker_pair_is_refused() {
        assert!(matches!(
            upsert_block(Some(&format!("x\n{BLOCK_BEGIN}\ny\n")), BODY),
            Err(BlockError::OneMarker)
        ));
        assert!(matches!(
            remove_block(&format!("{BLOCK_END}\n")),
            Err(BlockError::OneMarker)
        ));
        let two = format!("{BLOCK_BEGIN}\na\n{BLOCK_END}\n{BLOCK_BEGIN}\nb\n{BLOCK_END}\n");
        assert!(matches!(remove_block(&two), Err(BlockError::SeveralBlocks)));
    }

    #[test]
    fn the_markers_are_the_spec_text_and_a_file_without_a_block_is_no_change() {
        assert_eq!(
            BLOCK_BEGIN,
            "<!-- BEGIN CRYSTALLINE (managed by crystalline install; edits inside this block are replaced) -->"
        );
        assert_eq!(BLOCK_END, "<!-- END CRYSTALLINE -->");
        assert_eq!(remove_block("notes\n").unwrap(), None);
        let md = include_str!("../tests/fixtures/harness/gemini-md.md");
        let with = upsert_block(Some(md), BODY).unwrap().unwrap();
        assert!(with.starts_with(md) && with.ends_with(BLOCK_END));
        assert_eq!(remove_block(&with).unwrap().unwrap(), md);
    }
}
