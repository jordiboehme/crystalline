//! File space vs session space. The shared Y.Text must be LF-separated: the
//! client binding equates CodeMirror offsets with Y.Text UTF-16 offsets 1:1
//! and CM counts every line break as one unit, so a CRLF separator inside the
//! shared text corrupts the mapping. The file's own separator is recorded per
//! session and reapplied on save. The transform is exactly invertible for any
//! file `collab_eligible` admits; mixed-endings files are refused a session
//! rather than silently rewritten - fidelity is the contract.

/// The line separator a session records at load and reapplies at save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    CrLf,
    Lf,
}

impl Separator {
    /// The wire form carried in the hello control message ("\r\n" or "\n").
    pub fn as_str(&self) -> &'static str {
        match self {
            Separator::CrLf => "\r\n",
            Separator::Lf => "\n",
        }
    }
}

/// The same rule `lineSeparatorFor` applies client-side: CRLF when any CRLF
/// pair is present, LF otherwise (a lone \r is line content, never a separator).
pub fn separator_of(content: &str) -> Separator {
    if content.contains("\r\n") {
        Separator::CrLf
    } else {
        Separator::Lf
    }
}

/// Whether this content may host a collab session: LF files always; CRLF files
/// only when no lone \n exists outside a \r\n pair (the transform is only
/// invertible then) AND the LF session text would contain no literal "\r\n"
/// (a content \r before a separator would corrupt the client's CM<->Y.Text
/// offset mapping). Ineligible files fall back to solo editing.
pub fn collab_eligible(content: &str) -> bool {
    match separator_of(content) {
        Separator::Lf => true,
        // Two refusals for a CRLF file: a lone \n outside any pair (the save
        // transform would rewrite it to CRLF), and a content \r sitting right
        // before a separator (its LF session text would hold a literal
        // "\r\n", which the client's CM<->Y.Text offset mapping cannot
        // survive - a CM line break is one unit, that byte pair is two).
        Separator::CrLf => {
            !content.replace("\r\n", "").contains('\n') && !session_text(content).contains("\r\n")
        }
    }
}

/// File space -> session space: CRLF pairs become LF; LF files pass verbatim.
pub fn session_text(content: &str) -> String {
    match separator_of(content) {
        Separator::Lf => content.to_string(),
        Separator::CrLf => content.replace("\r\n", "\n"),
    }
}

/// Session space -> file space under the recorded separator. Exact inverse of
/// session_text for collab_eligible content.
pub fn file_text(session: &str, separator: Separator) -> String {
    match separator {
        Separator::Lf => session.to_string(),
        Separator::CrLf => session.replace('\n', "\r\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_follow_the_editor_rule() {
        assert_eq!(separator_of("a\r\nb"), Separator::CrLf);
        assert_eq!(separator_of("a\nb"), Separator::Lf);
        assert_eq!(separator_of("a\rb"), Separator::Lf, "a lone CR is content");
        assert_eq!(separator_of(""), Separator::Lf);
    }

    #[test]
    fn lf_files_pass_through_untouched() {
        let content = "---\ntitle: A\n---\n\nbody with a stray \r inside\n";
        assert!(collab_eligible(content));
        assert_eq!(session_text(content), content);
        assert_eq!(file_text(content, Separator::Lf), content);
    }

    #[test]
    fn crlf_files_round_trip_exactly() {
        for content in [
            "---\r\ntitle: A\r\n---\r\n\r\nbody\r\n",
            "no trailing newline\r\nlast line",
            "",
        ] {
            assert!(collab_eligible(content), "{content:?}");
            let session = session_text(content);
            assert!(
                !session.contains("\r\n"),
                "session space is LF: {session:?}"
            );
            assert_eq!(
                file_text(&session, separator_of(content)),
                content,
                "byte-identical round trip for {content:?}"
            );
        }
    }

    #[test]
    fn mixed_ending_files_are_refused_a_session() {
        // A CRLF file with a lone \n outside any pair: the transform would
        // rewrite that byte on save, so the file is not eligible.
        assert!(!collab_eligible("a\r\nmixed\nline\r\n"));
        // But a lone \n in an LF file is just the separator: eligible.
        assert!(collab_eligible("a\nb\n"));
        // "\r\r\n": the content \r sits right before a removed separator, so
        // LF session space would hold a literal "\r\n" - which corrupts the
        // client's CM<->Y.Text offset mapping (a CM line break is one unit,
        // that byte pair is two). Refused, not transformed.
        assert!(!collab_eligible("a\r\r\nb\r\n"));
        assert!(
            session_text("a\r\r\nb\r\n").contains("\r\n"),
            "the hazard is real"
        );
    }
}
