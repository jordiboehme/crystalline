//! OKF directory index files: the reserved filenames and the renderer for the
//! generated `index.md` a folder carries.
//!
//! OKF reserves `index.md` and `log.md` at every directory level: they are
//! never concept documents, so Crystalline excludes both names from sync,
//! indexing, search, verify and the watcher, and refuses to write an engram
//! under either name. `index.md` is a frontmatter-free directory listing
//! (`* [Title](relative-url) - description` entries under a section heading)
//! that lets a bundle be navigated statically, without Crystalline running.
//! Only a bundle-root index file may carry frontmatter, and only the
//! `okf_version` declaration.
//!
//! This module is pure: entries in, file contents out. Deciding when to
//! regenerate, walking the folder and writing the files is the service
//! engine's job.

/// The OKF version a bundle-root index file declares.
pub const OKF_VERSION: &str = "0.2";

/// The reserved directory index filename.
pub const INDEX_FILE: &str = "index.md";

/// The reserved directory log filename. Reserved (never indexed, never
/// written by Crystalline) but never generated either: OKF makes the log
/// fully optional.
pub const LOG_FILE: &str = "log.md";

/// The heading the generated index lists its entries under.
const CONTENTS_HEADING: &str = "# Contents";

/// Whether a bare filename is one of the OKF reserved names.
///
/// The match is exact and case-sensitive: OKF spells both names in lowercase,
/// so `index.md` is reserved while `Index.md` is an ordinary concept document
/// on every platform. That keeps one deterministic rule everywhere rather than
/// a rule that depends on the filesystem a domain happens to live on.
pub fn is_reserved_file(name: &str) -> bool {
    name == INDEX_FILE || name == LOG_FILE
}

/// Whether a forward-slashed, domain-relative path ends in a reserved
/// filename, at the root or at any level below it.
pub fn is_reserved_path(rel: &str) -> bool {
    rel.rsplit('/').next().is_some_and(is_reserved_file)
}

/// One line of a generated index: a link, its display title and an optional
/// description clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// The link target, relative to the folder holding the index file: a
    /// filename such as `setup.md` for a concept, or a trailing-slash name
    /// such as `runbooks/` for a subdirectory.
    pub link: String,
    /// The link text. A concept's `title` frontmatter, falling back to its
    /// filename stem; a subdirectory's name with its trailing slash.
    pub title: String,
    /// The concept's `description` frontmatter, when it has one. Rendered as
    /// a ` - description` clause and omitted entirely when absent or blank.
    pub description: Option<String>,
}

impl IndexEntry {
    /// An entry for a concept document in this folder.
    pub fn file(filename: &str, title: &str, description: Option<&str>) -> IndexEntry {
        let title = title.trim();
        let title = if title.is_empty() {
            filename.trim_end_matches(".md").to_string()
        } else {
            title.to_string()
        };
        IndexEntry {
            link: filename.to_string(),
            title,
            description: description.map(clean_description).filter(|d| !d.is_empty()),
        }
    }

    /// An entry for a subdirectory that holds concepts, linked with a trailing
    /// slash so a static viewer opens the folder (and its own index file).
    pub fn folder(name: &str) -> IndexEntry {
        let link = format!("{}/", name.trim_end_matches('/'));
        IndexEntry {
            title: link.clone(),
            link,
            description: None,
        }
    }
}

/// Render one folder's `index.md`. `root` marks the bundle root, the only
/// index file that carries frontmatter and the only place the `okf_version`
/// declaration may live.
///
/// Entries are sorted by link, so the same folder always renders byte for
/// byte the same file and a regeneration that changes nothing writes nothing.
/// Callers render only folders that hold at least one entry: a folder with no
/// concepts below it gets no index file at all.
pub fn render_index(entries: &[IndexEntry], root: bool) -> String {
    let mut sorted: Vec<&IndexEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.link.cmp(&b.link));

    let mut out = String::new();
    if root {
        out.push_str("---\n");
        out.push_str(&format!("okf_version: \"{OKF_VERSION}\"\n"));
        out.push_str("---\n\n");
    }
    out.push_str(CONTENTS_HEADING);
    out.push_str("\n\n");
    for entry in sorted {
        out.push_str(&format!(
            "* [{}]({})",
            escape_link_text(&entry.title),
            escape_link_url(&entry.link)
        ));
        if let Some(description) = &entry.description {
            out.push_str(&format!(" - {description}"));
        }
        out.push('\n');
    }
    out
}

/// Flatten a description onto one line: newlines and tabs become spaces, runs
/// of whitespace collapse and the ends are trimmed, so a multi-line
/// description never breaks the one-entry-per-line shape.
fn clean_description(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Escape the characters that would end a markdown link text early.
fn escape_link_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Render a link target. A target holding a space or a parenthesis is wrapped
/// in angle brackets, the markdown form that survives both.
fn escape_link_url(url: &str) -> String {
    if url.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names_match_exactly_and_at_every_level() {
        assert!(is_reserved_file("index.md"));
        assert!(is_reserved_file("log.md"));
        assert!(!is_reserved_file("Index.md"));
        assert!(!is_reserved_file("indexes.md"));
        assert!(!is_reserved_file("catalog.md"));

        assert!(is_reserved_path("index.md"));
        assert!(is_reserved_path("runbooks/log.md"));
        assert!(is_reserved_path("a/b/index.md"));
        assert!(!is_reserved_path("index.md/notes.md"));
        assert!(!is_reserved_path("runbooks/logs.md"));
    }

    #[test]
    fn a_root_index_declares_the_okf_version_and_lists_its_entries() {
        let entries = vec![
            IndexEntry::file("MANIFEST.md", "Ops", Some("How the team runs things")),
            IndexEntry::folder("runbooks"),
        ];
        assert_eq!(
            render_index(&entries, true),
            "---\nokf_version: \"0.2\"\n---\n\n\
             # Contents\n\n\
             * [Ops](MANIFEST.md) - How the team runs things\n\
             * [runbooks/](runbooks/)\n"
        );
    }

    #[test]
    fn a_non_root_index_carries_no_frontmatter() {
        let rendered = render_index(&[IndexEntry::file("setup.md", "Setup", None)], false);
        assert_eq!(rendered, "# Contents\n\n* [Setup](setup.md)\n");
        assert!(!rendered.starts_with("---"));
    }

    #[test]
    fn a_missing_title_falls_back_to_the_filename_stem() {
        let entry = IndexEntry::file("deploy-notes.md", "  ", None);
        assert_eq!(entry.title, "deploy-notes");
        assert_eq!(
            render_index(&[entry], false),
            "# Contents\n\n* [deploy-notes](deploy-notes.md)\n"
        );
    }

    #[test]
    fn a_blank_description_drops_the_clause_and_a_long_one_stays_on_one_line() {
        assert_eq!(IndexEntry::file("a.md", "A", Some("   ")).description, None);
        assert_eq!(
            IndexEntry::file("a.md", "A", Some("first line\n  second line"))
                .description
                .as_deref(),
            Some("first line second line")
        );
    }

    #[test]
    fn entries_are_sorted_by_link_so_the_render_is_deterministic() {
        let entries = vec![
            IndexEntry::folder("zulu"),
            IndexEntry::file("beta.md", "Beta", None),
            IndexEntry::file("alpha.md", "Alpha", None),
            IndexEntry::folder("alpha-dir"),
        ];
        let rendered = render_index(&entries, false);
        assert_eq!(
            rendered,
            // Byte order on the link, so `alpha-dir/` precedes `alpha.md`.
            "# Contents\n\n\
             * [alpha-dir/](alpha-dir/)\n\
             * [Alpha](alpha.md)\n\
             * [Beta](beta.md)\n\
             * [zulu/](zulu/)\n"
        );
        // The same entries in any order render the same bytes.
        let mut shuffled = entries;
        shuffled.reverse();
        assert_eq!(render_index(&shuffled, false), rendered);
    }

    #[test]
    fn brackets_in_a_title_and_spaces_in_a_filename_stay_inside_the_link() {
        let rendered = render_index(
            &[IndexEntry::file("my notes.md", "Notes [draft]", None)],
            false,
        );
        assert_eq!(
            rendered,
            "# Contents\n\n* [Notes \\[draft\\]](<my notes.md>)\n"
        );
    }

    #[test]
    fn the_render_has_no_trailing_whitespace_and_ends_with_one_newline() {
        let rendered = render_index(
            &[
                IndexEntry::file("a.md", "A", Some("desc")),
                IndexEntry::folder("sub"),
            ],
            true,
        );
        for line in rendered.lines() {
            assert_eq!(line, line.trim_end(), "trailing whitespace in {line:?}");
        }
        assert!(rendered.ends_with('\n'));
        assert!(!rendered.ends_with("\n\n"));
    }
}
