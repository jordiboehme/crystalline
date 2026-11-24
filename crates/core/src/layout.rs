//! What every path in a domain is, decided in one place.
//!
//! A domain folder holds engrams, the attachment folder `assets/`, the OKF
//! reserved names, hidden files, the artifact folders its MANIFEST
//! provisions from inside the root and, while its Reflection is in effect,
//! the Reflection's own folder `reflections/`. Every walk of a domain - the sync,
//! verify, the folder indexes, doctor, export and import, share changes and
//! the watcher - asks [`DomainLayout::classify`] what a path is and keeps its
//! own policy over the answer, so a new kind of content is taught to every
//! walk at once instead of to each walk by hand.

use std::path::Path;

use crate::kind::ContentKind;
use crate::reflection::ReflectionFile;

/// Whether a domain keeps a Reflection that decides what its `reflections/`
/// folder is. `Off` makes the folder ordinary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReflectionMode {
    /// The domain keeps no Reflection.
    #[default]
    Off,
    /// The domain's Reflection is in effect: `reflections/` is its own.
    Effective,
}

impl ReflectionMode {
    /// The declared switch read as a mode, for a walk that has no index to
    /// tell a pending switch from an effective one (verify run on its own,
    /// the CLI hooks). A walk with an index passes the switch in effect.
    pub fn declared(switch: crate::manifest::ReflectionSwitch) -> ReflectionMode {
        match switch {
            crate::manifest::ReflectionSwitch::On => ReflectionMode::Effective,
            crate::manifest::ReflectionSwitch::Off => ReflectionMode::Off,
        }
    }
}

/// What one domain-relative path is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathClass {
    /// A markdown file the engram side reads.
    Engram,
    /// A file under the reserved `assets/` folder, whatever its extension.
    EngramAsset,
    /// An OKF reserved name (`index.md`, `log.md`) at any level.
    Reserved,
    /// A path with a hidden segment.
    Hidden,
    /// A path inside an artifact folder the MANIFEST provisions from inside
    /// the domain root.
    Artifact,
    /// Anything else: a file that is not markdown and not under `assets/`.
    Other,
    /// A file inside the domain's Reflection, while it is in effect.
    Reflection(ReflectionFile),
}

impl PathClass {
    /// The kind of document the path belongs to, if any.
    pub fn kind(&self) -> Option<ContentKind> {
        match self {
            PathClass::Engram | PathClass::EngramAsset => Some(ContentKind::Engram),
            PathClass::Reserved | PathClass::Hidden | PathClass::Artifact | PathClass::Other => {
                None
            }
            PathClass::Reflection(_) => Some(ContentKind::Reflection),
        }
    }

    /// Whether the path may travel with an engram share. Every class but the
    /// Reflection's may; a share's own rules (hidden paths, `log.md`,
    /// generated indexes) narrow the rest.
    pub fn shares(&self) -> bool {
        match self {
            PathClass::Engram
            | PathClass::EngramAsset
            | PathClass::Reserved
            | PathClass::Hidden
            | PathClass::Artifact
            | PathClass::Other => true,
            // The Reflection has its own home and its own backup branch; a
            // share carries engrams only.
            PathClass::Reflection(_) => false,
        }
    }

    /// Whether the path belongs in the backup of a domain's own documents,
    /// which is kept apart from the engram share: the Reflection's pages,
    /// pact, threads, baseline and files under `reflections/assets/`, all
    /// but its temp and stray files.
    pub fn backs_up(&self) -> bool {
        match self {
            PathClass::Engram
            | PathClass::EngramAsset
            | PathClass::Reserved
            | PathClass::Hidden
            | PathClass::Artifact
            | PathClass::Other => false,
            PathClass::Reflection(file) => match file {
                ReflectionFile::Page(_)
                | ReflectionFile::Pact
                | ReflectionFile::PagePact { .. }
                | ReflectionFile::Threads { .. }
                | ReflectionFile::Baseline { .. }
                | ReflectionFile::Asset(_) => true,
                ReflectionFile::Other => false,
            },
        }
    }
}

/// How one domain's folder is laid out: its in-root artifact folders and its
/// Reflection mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainLayout {
    /// The in-root artifact folders, domain-relative and forward-slashed.
    artifacts: Vec<String>,
    reflection: ReflectionMode,
}

impl DomainLayout {
    /// The layout of the domain rooted at `root`: the artifact folders its
    /// MANIFEST provisions from inside the root (none when the MANIFEST is
    /// missing or unparseable), and the Reflection mode the caller knows.
    pub fn at(root: &Path, reflection: ReflectionMode) -> DomainLayout {
        let artifacts = crate::manifest::in_root_artifact_dirs(root)
            .iter()
            .map(|dir| slash_rel(root, dir))
            .filter(|rel| !rel.is_empty())
            .collect();
        DomainLayout {
            artifacts,
            reflection,
        }
    }

    /// A layout without artifact folders, for a walk whose policy treats them
    /// like the rest of the domain (share changes, the watcher, a walk of a
    /// folder that is not a domain yet). Reads nothing.
    pub fn bare(reflection: ReflectionMode) -> DomainLayout {
        DomainLayout {
            artifacts: Vec::new(),
            reflection,
        }
    }

    /// The Reflection mode this layout was built with.
    pub fn reflection(&self) -> ReflectionMode {
        self.reflection
    }

    /// What `rel` (domain-relative, forward-slashed) is. While the
    /// Reflection is in effect, everything under `reflections/` (the folder
    /// matched with ASCII case folded) is the Reflection's, hidden files
    /// included, so its baseline under `.reflected/` is classified rather
    /// than hidden. Below that the precedence is hidden, reserved, artifact,
    /// attachment, markdown: the order every walk applied its own filters in
    /// before they asked here.
    pub fn classify(&self, rel: &str) -> PathClass {
        let rel = rel.trim_start_matches("./");
        if self.reflection == ReflectionMode::Effective
            && crate::reflection::is_under_reflections(rel)
        {
            let inner = rel.split_once('/').map(|(_, rest)| rest).unwrap_or("");
            return PathClass::Reflection(crate::reflection::classify(inner));
        }
        if rel.split('/').any(is_hidden_segment) {
            return PathClass::Hidden;
        }
        if crate::index::is_reserved_path(rel) {
            return PathClass::Reserved;
        }
        if self.in_artifact(rel) {
            return PathClass::Artifact;
        }
        if crate::attachment::is_under_assets(rel) {
            return PathClass::EngramAsset;
        }
        if rel.to_ascii_lowercase().ends_with(".md") {
            PathClass::Engram
        } else {
            PathClass::Other
        }
    }

    /// Whether a walk that reads this domain's own files descends into the
    /// directory at `rel`: not into a directory with a hidden segment, not
    /// into an artifact folder. That holds inside the Reflection too, so no
    /// such walk descends into its baseline under `.reflected/`.
    pub fn enters_dir(&self, rel: &str) -> bool {
        let rel = rel.trim_start_matches("./");
        !rel.split('/').any(is_hidden_segment) && !self.in_artifact(rel)
    }

    /// Whether a walk over the engrams alone (verify, the folder indexes,
    /// doctor) prunes the directory at `rel`: what [`Self::enters_dir`]
    /// leaves out, and the Reflection's folder itself while it is in effect.
    pub fn prunes_dir(&self, rel: &str) -> bool {
        !self.enters_dir(rel)
            || (self.reflection == ReflectionMode::Effective
                && crate::reflection::is_under_reflections(rel))
    }

    fn in_artifact(&self, rel: &str) -> bool {
        self.artifacts.iter().any(|dir| {
            rel == dir
                || rel
                    .strip_prefix(dir.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

/// Whether one path segment is hidden: a dot-file or dot-directory.
fn is_hidden_segment(segment: &str) -> bool {
    segment.starts_with('.') && segment != "." && segment != ".."
}

/// `path` relative to `root`, forward-slashed: the key every walker uses.
pub fn slash_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// What the caller of [`is_pending_engram`] knows about the index row of
/// the path it asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngramRow {
    /// The index records an engram row for the path.
    Recorded,
    /// No row is known: the index holds nothing of the domain, and the
    /// files on disk are asked instead.
    Unknown,
}

/// Whether the file at `rel` (domain-relative, under `reflections/`) in the
/// domain at `root` is an engram that waits to move out of the Reflection's
/// folder: the walk would take it for an engram (the class with the
/// Reflection off) and it carries no page marker. A page marker is a section id line `<!-- r:<hex> -->`, the
/// Reflection's provenance key in the frontmatter, or a baseline twin under
/// `reflections/.reflected/`; a threads file (`<page>.threads.md`) is marked
/// when its page is a marked page. Crystalline writes an id line under every
/// `##` heading of a page it writes, the seeded `PACT.md` included, so a
/// domain switched off and on again finds no pending engram among them; a
/// page a person wrote by hand with none of the markers, or one with no `##`
/// heading and no provenance, reads as an engram, which only holds the
/// switch pending (no file is ever dropped).
///
/// A file that cannot be read counts as pending (fail closed). What a file
/// that no longer reads as an engram (not valid UTF-8, or frontmatter that
/// does not parse) means depends on `row`: with an engram row recorded for
/// the path, the walk fails on the file and keeps the row of its last good
/// version, so the file counts as pending unless it carries a page marker
/// (a file that is not UTF-8 has none to show); without one (the walk over
/// the disk when the index holds nothing) it is no engram, as the walk sees
/// it. A file that is gone is no engram. The class is taken from
/// [`DomainLayout::bare`], which knows no artifact folder, so a markdown file
/// in an in-root artifact folder under `reflections/` may be reported
/// pending although the walk skips it: the safe direction.
///
/// This is the one predicate for "an engram under `reflections/`": the sync
/// asks it to decide whether the switch is in effect, and the switch-on
/// screen names exactly the files it calls pending.
pub fn is_pending_engram(root: &Path, rel: &str, row: EngramRow) -> bool {
    let rel = rel.trim_start_matches("./");
    let Some((folder, inner)) = rel.split_once('/') else {
        return false;
    };
    if !folder.eq_ignore_ascii_case(crate::reflection::REFLECTIONS_DIR) {
        return false;
    }
    if DomainLayout::bare(ReflectionMode::Off).classify(rel) != PathClass::Engram {
        return false;
    }
    let bytes = match std::fs::read(root.join(rel)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return row == EngramRow::Recorded;
    };
    let text = crate::to_lf(&text);
    if row == EngramRow::Unknown && crate::parse_engram(&text).is_err() {
        return false;
    }
    let folder = root.join(folder);
    match crate::reflection::classify(inner) {
        // A threads file is the page's when its page is a marked page; one
        // beside an unmarked or unreadable file stays an engram.
        ReflectionFile::Threads { of } => !page_marked(&folder, &of),
        // A page pact is the page's as its threads are, and it is marked by
        // its own id lines or baseline twin too.
        ReflectionFile::PagePact { of } => {
            !carries_page_marker(&text)
                && !has_baseline_twin(&folder, inner)
                && !page_marked(&folder, &of)
        }
        _ => !carries_page_marker(&text) && !has_baseline_twin(&folder, inner),
    }
}

/// Whether the page at `inner` in the Reflection's `folder` carries a page
/// marker: in its own text, or as a baseline twin. A page that cannot be read
/// is unmarked, which keeps its threads file pending.
fn page_marked(folder: &Path, inner: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(folder.join(inner)) else {
        return false;
    };
    carries_page_marker(&crate::to_lf(&text)) || has_baseline_twin(folder, inner)
}

/// Whether `inner` has a baseline twin under the folder's `.reflected/`. A
/// twin that cannot be stat'd is taken as absent, which keeps the file
/// pending.
fn has_baseline_twin(folder: &Path, inner: &str) -> bool {
    folder
        .join(crate::reflection::baseline_path(inner))
        .try_exists()
        .unwrap_or(false)
}

/// Whether `source` carries a page marker in its own text: the Reflection's
/// provenance key in the frontmatter, or a section id line, which is the line
/// right under a `##` heading outside a fenced code block (the sections
/// reader's rule), so an engram that shows the format in a fence is no page.
fn carries_page_marker(source: &str) -> bool {
    if let Some(parts) = crate::frontmatter::split_frontmatter(source)
        && let Ok(serde_yaml_ng::Value::Mapping(map)) =
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(parts.yaml)
        && map.contains_key(crate::reflection::PROVENANCE_KEY)
    {
        return true;
    }
    // The first section is the top, which always answers its fixed id.
    crate::reflection::split_sections(source)
        .iter()
        .skip(1)
        .any(|section| section.id.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(artifacts: &[&str]) -> DomainLayout {
        DomainLayout {
            artifacts: artifacts.iter().map(|a| a.to_string()).collect(),
            reflection: ReflectionMode::Off,
        }
    }

    /// Today's rules, one row each: the class every walker on main already
    /// derives by hand. Every walker routes through this table, so a row
    /// that moves here moves a walker.
    #[test]
    fn the_classes_of_todays_rules() {
        let layout = layout(&["skills", "deploy/out"]);
        for (rel, class) in [
            ("notes/a.md", PathClass::Engram),
            ("notes/A.MD", PathClass::Engram),
            ("MANIFEST.md", PathClass::Engram),
            ("./notes/a.md", PathClass::Engram),
            ("reflections/a.md", PathClass::Engram),
            ("Index.md", PathClass::Engram),
            ("assets-notes/a.md", PathClass::Engram),
            ("skillset/a.md", PathClass::Engram),
            ("deploy/a.md", PathClass::Engram),
            ("assets/shot.png", PathClass::EngramAsset),
            ("Assets/shot.png", PathClass::EngramAsset),
            ("assets/notes.md", PathClass::EngramAsset),
            ("assets", PathClass::EngramAsset),
            ("index.md", PathClass::Reserved),
            ("runbooks/log.md", PathClass::Reserved),
            ("assets/index.md", PathClass::Reserved),
            ("skills/index.md", PathClass::Reserved),
            (".git/config", PathClass::Hidden),
            ("notes/.draft.md", PathClass::Hidden),
            (".crystalline.yaml", PathClass::Hidden),
            ("assets/.tmp.png", PathClass::Hidden),
            ("skills", PathClass::Artifact),
            ("skills/x/SKILL.md", PathClass::Artifact),
            ("deploy/out/a.md", PathClass::Artifact),
            ("notes/picture.png", PathClass::Other),
        ] {
            assert_eq!(layout.classify(rel), class, "{rel}");
        }
    }

    #[test]
    fn a_walk_enters_every_folder_but_hidden_and_artifact_ones() {
        let layout = layout(&["skills"]);
        for dir in ["notes", "assets", "Assets", "reflections"] {
            assert!(layout.enters_dir(dir), "{dir}");
            assert!(!layout.prunes_dir(dir), "{dir}");
        }
        for dir in [".git", "notes/.trash", "skills", "skills/deep"] {
            assert!(!layout.enters_dir(dir), "{dir}");
            assert!(layout.prunes_dir(dir), "{dir}");
        }
    }

    #[test]
    fn every_engram_side_class_shares_and_none_backs_up() {
        for class in [
            PathClass::Engram,
            PathClass::EngramAsset,
            PathClass::Reserved,
            PathClass::Hidden,
            PathClass::Artifact,
            PathClass::Other,
        ] {
            assert!(class.shares(), "{class:?}");
            assert!(!class.backs_up(), "{class:?}");
        }
        assert_eq!(PathClass::Engram.kind(), Some(ContentKind::Engram));
        assert_eq!(PathClass::EngramAsset.kind(), Some(ContentKind::Engram));
        assert_eq!(PathClass::Other.kind(), None);
    }

    #[test]
    fn the_layout_reads_the_artifact_folders_from_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(
            DomainLayout::at(root, ReflectionMode::Off),
            DomainLayout::bare(ReflectionMode::Off),
            "no MANIFEST"
        );
        std::fs::write(
            root.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: KB\npermalink: manifest\ntags: [manifest]\nstatus: stable\n---\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n\n## Provisioning\n\n- skills: skills\n- elsewhere: ../outside\n",
        )
        .unwrap();
        let layout = DomainLayout::at(root, ReflectionMode::Off);
        assert_eq!(layout.classify("skills/x/SKILL.md"), PathClass::Artifact);
        assert_eq!(
            layout.classify("notes/a.md"),
            PathClass::Engram,
            "an out-of-root decl names no folder here"
        );
        assert_eq!(
            DomainLayout::bare(ReflectionMode::Off).classify("skills/x/SKILL.md"),
            PathClass::Engram
        );
        assert_eq!(
            slash_rel(root, &root.join("skills").join("x").join("SKILL.md")),
            "skills/x/SKILL.md"
        );
    }

    #[test]
    fn a_reflection_folder_is_its_own_while_the_mode_is_effective() {
        use crate::reflection::ReflectionFile;
        let on = DomainLayout::bare(ReflectionMode::Effective);
        for (rel, file) in [
            (
                "reflections/guides/a.md",
                ReflectionFile::Page("guides/a.md".into()),
            ),
            ("Reflections/a.md", ReflectionFile::Page("a.md".into())),
            ("reflections/PACT.md", ReflectionFile::Pact),
            (
                "reflections/a.threads.md",
                ReflectionFile::Threads { of: "a.md".into() },
            ),
            (
                "reflections/.reflected/a.md",
                ReflectionFile::Baseline { of: "a.md".into() },
            ),
            ("reflections/.a.md.tmp", ReflectionFile::Other),
            ("reflections/index.md", ReflectionFile::Other),
            (
                "reflections/assets/flow.png",
                ReflectionFile::Asset("assets/flow.png".into()),
            ),
            ("reflections/assets/a.md", ReflectionFile::Other),
        ] {
            assert_eq!(on.classify(rel), PathClass::Reflection(file), "{rel}");
        }
        assert_eq!(
            on.classify("notes/reflections/a.md"),
            PathClass::Engram,
            "only at the root"
        );
        assert_eq!(on.classify("reflectionsx/a.md"), PathClass::Engram);
        let off = DomainLayout::bare(ReflectionMode::Off);
        assert_eq!(
            off.classify("reflections/a.md"),
            PathClass::Engram,
            "an ordinary folder while off"
        );
        assert!(on.prunes_dir("reflections") && on.prunes_dir("Reflections"));
        assert!(on.enters_dir("reflections") && !on.enters_dir("reflections/.reflected"));
        assert!(!off.prunes_dir("reflections"));
        let page = on.classify("reflections/a.md");
        assert_eq!(
            (page.kind(), page.shares(), page.backs_up()),
            (Some(ContentKind::Reflection), false, true)
        );
        assert!(
            !on.classify("reflections/.a.md.tmp").backs_up(),
            "a temp file is never backed up"
        );
        assert!(
            on.classify("reflections/assets/flow.png").backs_up(),
            "a Reflection's file is backed up with its pages"
        );
        assert_eq!(
            ReflectionMode::declared(crate::manifest::ReflectionSwitch::On),
            ReflectionMode::Effective
        );
        assert_eq!(
            ReflectionMode::declared(crate::manifest::ReflectionSwitch::Off),
            ReflectionMode::Off
        );
    }

    /// R48: what counts as an engram waiting to move out of `reflections/`.
    #[test]
    fn a_pending_engram_is_an_engram_without_a_page_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let put = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put("reflections/plain.md", "# Plain\n\nA note.\n");
        put(
            "reflections/flow.md",
            "---\n{type: engram, status: draft}\n---\n\n# Flow\n",
        );
        put(
            "reflections/ids.md",
            "# Page\n\n## Part\n<!-- r:aaaa1111 -->\n\nText.\n",
        );
        put(
            "reflections/prov.md",
            "---\ntype: guide\nstatus: draft\nreflection: {}\n---\n\n# Prov\n",
        );
        put("reflections/twin.md", "# Twin\n");
        put("reflections/.reflected/twin.md", "# Twin\n");
        put("reflections/twin.threads.md", "# Threads\n");
        put("reflections/lone.threads.md", "# Threads\n");
        put("reflections/broken.md", "---\n: [\n---\n");
        put("reflections/notes.txt", "text");
        put("notes/a.md", "# A\n");
        for (rel, pending) in [
            ("reflections/plain.md", true),
            ("Reflections/plain.md", true),
            ("reflections/flow.md", true),
            ("reflections/lone.threads.md", true),
            ("reflections/ids.md", false),
            ("reflections/prov.md", false),
            ("reflections/twin.md", false),
            ("reflections/twin.threads.md", false),
            ("reflections/broken.md", false),
            ("reflections/notes.txt", false),
            ("reflections/gone.md", false),
            ("notes/a.md", false),
        ] {
            // `Reflections/plain.md` only resolves on a case-folding filesystem.
            if rel.starts_with('R') && !root.join(rel).exists() {
                continue;
            }
            assert_eq!(
                is_pending_engram(root, rel, EngramRow::Unknown),
                pending,
                "{rel}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("reflections/ids.md");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = std::fs::read(&path).is_err();
            let pending = is_pending_engram(root, "reflections/ids.md", EngramRow::Unknown);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            if unreadable {
                assert!(pending, "a file that cannot be read counts as pending");
            }
        }
    }

    /// R48a: a file that no longer reads as an engram (frontmatter that does
    /// not parse, bytes that are not UTF-8) is pending only where the index
    /// records an engram row for it; a page marker still clears it.
    #[test]
    fn a_file_that_stopped_reading_as_an_engram_is_pending_only_with_a_row() {
        let dir = domain_with(&[
            ("reflections/broken.md", "---\n: [\n---\n\n# Broken\n"),
            (
                "reflections/marked.md",
                "---\n: [\n---\n\n# Page\n\n## Part\n<!-- r:aaaa1111 -->\n\nText.\n",
            ),
        ]);
        std::fs::write(dir.path().join("reflections/latin.md"), b"# Caf\xe9\n").unwrap();
        for (rel, recorded, unknown) in [
            ("reflections/broken.md", true, false),
            ("reflections/latin.md", true, false),
            ("reflections/marked.md", false, false),
        ] {
            assert_eq!(
                is_pending_engram(dir.path(), rel, EngramRow::Recorded),
                recorded,
                "{rel} with a row"
            );
            assert_eq!(
                is_pending_engram(dir.path(), rel, EngramRow::Unknown),
                unknown,
                "{rel} without one"
            );
        }
    }

    fn domain_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    /// A threads file is the page's only when its page is a marked page: one
    /// beside a file that does not parse, or beside an unmarked engram,
    /// stays an engram.
    #[test]
    fn a_threads_file_is_marked_by_a_marked_page_only() {
        let dir = domain_with(&[
            ("reflections/broken.md", "---\n: [\n---\n"),
            ("reflections/broken.threads.md", "# Threads\n\nMine.\n"),
            ("reflections/note.md", "# Note\n"),
            ("reflections/note.threads.md", "# Threads\n"),
            (
                "reflections/page.md",
                "# Page\n\n## Part\n<!-- r:aaaa1111 -->\n\nText.\n",
            ),
            ("reflections/page.threads.md", "# Threads\n"),
            ("reflections/twin.md", "# Twin\n"),
            ("reflections/.reflected/twin.md", "# Twin\n"),
            ("reflections/twin.threads.md", "# Threads\n"),
        ]);
        for (rel, pending) in [
            ("reflections/broken.threads.md", true),
            ("reflections/note.threads.md", true),
            ("reflections/page.threads.md", false),
            ("reflections/twin.threads.md", false),
        ] {
            assert_eq!(
                is_pending_engram(dir.path(), rel, EngramRow::Unknown),
                pending,
                "{rel}"
            );
        }
    }

    /// A page pact is the page's as a threads file is: marked by its own
    /// id lines or baseline twin, or by its page being a marked page. A
    /// hand-written pact beside a marked page needs no id line of its own;
    /// one beside an unmarked engram stays an engram.
    #[test]
    fn a_page_pact_is_marked_by_itself_or_by_a_marked_page() {
        let dir = domain_with(&[
            (
                "reflections/page.md",
                "# Page\n\n## Part\n<!-- r:aaaa1111 -->\n\nText.\n",
            ),
            ("reflections/page.pact.md", "# Pact\n\nBe short.\n"),
            ("reflections/note.md", "# Note\n"),
            ("reflections/note.pact.md", "# Pact\n"),
            (
                "reflections/own.pact.md",
                "# Pact\n\n## Tone\n<!-- r:bbbb2222 -->\n\nShort.\n",
            ),
        ]);
        for (rel, pending) in [
            ("reflections/page.pact.md", false),
            ("reflections/note.pact.md", true),
            ("reflections/own.pact.md", false),
        ] {
            assert_eq!(
                is_pending_engram(dir.path(), rel, EngramRow::Unknown),
                pending,
                "{rel}"
            );
        }
        let on = DomainLayout::bare(ReflectionMode::Effective);
        let class = on.classify("reflections/page.pact.md");
        assert_eq!(
            class,
            PathClass::Reflection(ReflectionFile::PagePact {
                of: "page.md".into()
            })
        );
        assert!(class.backs_up(), "a page pact travels with its page");
    }

    /// Only an id line right under a `##` heading outside a fence marks a
    /// page: an engram that shows the format in a code block, or has an
    /// id-shaped line in its prose, stays an engram.
    #[test]
    fn only_a_section_id_line_marks_a_page() {
        let dir = domain_with(&[
            (
                "reflections/fenced.md",
                "# Format\n\n```markdown\n## Part\n<!-- r:cafe1234 -->\n```\n",
            ),
            (
                "reflections/loose.md",
                "# Loose\n\nText.\n<!-- r:cafe1234 -->\n",
            ),
            (
                "reflections/page.md",
                "# Page\n\n## Part\n<!-- r:cafe1234 -->\n\nText.\n",
            ),
        ]);
        for (rel, pending) in [
            ("reflections/fenced.md", true),
            ("reflections/loose.md", true),
            ("reflections/page.md", false),
        ] {
            assert_eq!(
                is_pending_engram(dir.path(), rel, EngramRow::Unknown),
                pending,
                "{rel}"
            );
        }
    }
}
