//! E-family rules: frontmatter format and encoding, plus one rule about the
//! domain's file paths themselves.
//!
//! `E001`-`E006` are unconditional errors: a well-formed Engram must parse,
//! carry the four required fields and use a lowercase slug permalink with
//! clean UTF-8. `E007` (tag format) and the outside-the-recommended-set half
//! of `E003` are softer: Crystalline never enforces a closed `type` or
//! `status` vocabulary, so those two only ever inform. `E008` warns when a
//! permalink starts with the domain's own name: a permalink is
//! domain-relative (the OKF Concept ID made explicit) and the domain name is
//! per-user configuration, so persisting it into a file misleads as soon as
//! the domain is registered under another name.
//!
//! `E009` is the odd one out twice over: it is about paths rather than
//! frontmatter, and it is domain-scoped rather than per-file, so it runs from
//! [`check_domain`] beside the other domain-level families instead of from
//! [`check`]. Nothing a single document contains can tell you that another
//! document's path collides with it.

use std::collections::BTreeMap;

use crate::address::slugify;
use crate::engram::RECOMMENDED_TYPES;
use crate::parse::ParseError;

use super::scanner::{Domain, ScannedFile};
use super::{Severity, Sink};

pub(crate) fn check(file: &ScannedFile, domain_name: &str, sink: &mut Sink) {
    let engram = match &file.parsed {
        Ok(e) => e,
        Err(ParseError::Bom) => {
            sink.emit(
                &file.path,
                None,
                "E006",
                Severity::Error,
                "file starts with a UTF-8 byte order mark",
                Some("save the file as UTF-8 without a BOM".into()),
            );
            return;
        }
        Err(ParseError::NullByte { line, .. }) => {
            sink.emit(
                &file.path,
                Some(*line),
                "E006",
                Severity::Error,
                "file contains a null byte",
                Some("remove the null byte and re-save as plain UTF-8 text".into()),
            );
            return;
        }
        Err(ParseError::Yaml { message }) => {
            sink.emit(
                &file.path,
                None,
                "E001",
                Severity::Error,
                format!("frontmatter YAML is invalid: {message}"),
                None,
            );
            return;
        }
        Err(ParseError::FrontmatterNotMapping) => {
            sink.emit(
                &file.path,
                None,
                "E001",
                Severity::Error,
                "frontmatter is not a mapping",
                None,
            );
            return;
        }
    };

    let fm = &engram.frontmatter;

    if fm.title.trim().is_empty() {
        sink.emit(
            &file.path,
            None,
            "E002",
            Severity::Error,
            "required field `title` is missing",
            None,
        );
    }
    if fm
        .permalink
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        sink.emit(
            &file.path,
            None,
            "E002",
            Severity::Error,
            "required field `permalink` is missing",
            None,
        );
    }

    if fm.engram_type.trim().is_empty() {
        sink.emit(
            &file.path,
            None,
            "E003",
            Severity::Error,
            "required field `type` is missing",
            None,
        );
    } else if !RECOMMENDED_TYPES.contains(&fm.engram_type.as_str()) {
        sink.emit(
            &file.path,
            None,
            "E003",
            Severity::Info,
            format!("type `{}` is outside the recommended set", fm.engram_type),
            None,
        );
    }

    if fm.tags.is_empty() {
        sink.emit(
            &file.path,
            None,
            "E004",
            Severity::Error,
            "required field `tags` is missing or empty",
            None,
        );
    }

    if let Some(p) = &fm.permalink
        && !p.is_empty()
        && slugify(p) != *p
    {
        sink.emit(
            &file.path,
            None,
            "E005",
            Severity::Error,
            format!("permalink `{p}` is not a lowercase slug path"),
            Some(format!("use `{}`", slugify(p))),
        );
    }

    // E008: a permalink that opens with the domain's own name persists
    // per-user configuration into content. Exempt when the whole permalink
    // is just the file's own path slug: then the leading segment is a real
    // subfolder that happens to share the name, correct by path.
    if let Some(p) = &fm.permalink
        && let Some(rest) = p.strip_prefix(&format!("{}/", slugify(domain_name)))
        && !rest.is_empty()
        && *p != slugify(&file.rel_path.to_string_lossy())
    {
        sink.emit(
            &file.path,
            None,
            "E008",
            Severity::Warning,
            format!(
                "permalink `{p}` starts with the domain name; permalinks are domain-relative and the domain name is per-user configuration"
            ),
            Some(format!("use `{rest}`")),
        );
    }

    for tag in &fm.tags {
        if !crate::tags::is_lower_hyphen(tag) {
            sink.emit(
                &file.path,
                None,
                "E007",
                Severity::Warning,
                format!("tag `{tag}` is not lowercase-with-hyphens"),
                None,
            );
        }
    }
}

/// `E009`: two or more of the domain's paths differ from each other only in
/// case.
///
/// Git and a case-sensitive filesystem hold `Classes/Common/a.md` and
/// `classes/common/a.md` as two files; macOS APFS and Windows NTFS hold one.
/// So a domain carrying both spellings cannot be checked out intact on either
/// platform, and the member who tries gets one file where the repository has
/// two, with no error to say so.
///
/// It misleads local change detection as well.
/// `crystalline_remote::changes::detect_local_changes` folds a walked path
/// onto a recorded one whose only difference is case, but only where exactly
/// one recorded path folds that way. A domain holding both spellings is the
/// ambiguous half it declines to guess at: on a case-insensitive filesystem it
/// cannot tell the checked-out file from the spelling that was left behind, so
/// it reports neither rather than offer to delete the one the user can see.
/// Renaming one of the pair is the fix for both problems, which is what this
/// rule asks for.
///
/// The folding here must match `crystalline_remote::changes::fold_case`, which
/// asks the same question of a domain's paths on the sharing side. `core` may
/// not depend on that crate, so the two `to_lowercase` calls are kept in step
/// by hand, and each says so.
pub(crate) fn check_domain(domain: &Domain, sink: &mut Sink) {
    // `domain.files` is sorted by path, so each group's paths come out in a
    // stable order and the reported message does not depend on walk order.
    let mut folded: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, file) in domain.files.iter().enumerate() {
        folded
            .entry(file.rel_path.to_string_lossy().to_lowercase())
            .or_default()
            .push(i);
    }

    for group in folded.values().filter(|group| group.len() > 1) {
        let paths: Vec<String> = group
            .iter()
            .map(|i| domain.files[*i].rel_path.to_string_lossy().into_owned())
            .collect();
        let others = paths[1..].join("`, `");
        sink.emit(
            &domain.files[group[0]].path,
            None,
            "E009",
            Severity::Error,
            format!(
                "path `{}` differs only in case from `{others}`; no macOS or Windows checkout can hold them all, so all but one disappear there without warning",
                paths[0]
            ),
            Some("rename one of them so the paths differ by more than case".into()),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::verify::Issue;
    use crate::verify::scanner::scanned_file_from_source;

    const DOC: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA body.\n";

    /// A domain over synthetic relative paths and no filesystem at all. That
    /// is the point: the pair `E009` reports cannot be written to a macOS or
    /// Windows temp directory, which is exactly why the rule exists, so the
    /// rule's own tests must not need one.
    fn findings(rel_paths: &[&str]) -> Vec<Issue> {
        let domain = Domain {
            name: "classes".to_string(),
            root: PathBuf::from("classes"),
            manifest_index: None,
            files: rel_paths
                .iter()
                .map(|p| scanned_file_from_source(Path::new(p), DOC))
                .collect(),
            config: crate::config::DomainConfig::default(),
        };
        let mut issues = Vec::new();
        let mut summary = crate::verify::Summary::default();
        let mut sink = Sink::new(&mut issues, &mut summary, None, false);
        check_domain(&domain, &mut sink);
        issues
    }

    #[test]
    fn two_paths_differing_only_in_case_are_one_e009_naming_both() {
        let issues = findings(&[
            "classes/Platform.Components.Common/CustomHeaderModule.md",
            "classes/platform.components.common/CustomHeaderModule.md",
        ]);

        assert_eq!(issues.len(), 1, "{issues:#?}");
        let issue = &issues[0];
        assert_eq!(issue.rule, "E009");
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(
            issue.message,
            "path `classes/Platform.Components.Common/CustomHeaderModule.md` differs only in case from `classes/platform.components.common/CustomHeaderModule.md`; no macOS or Windows checkout can hold them all, so all but one disappear there without warning"
        );
        assert_eq!(
            issue.fix.as_deref(),
            Some("rename one of them so the paths differ by more than case")
        );
    }

    #[test]
    fn a_domain_with_no_case_collision_reports_nothing() {
        let issues = findings(&[
            "classes/Platform.Components.Common/CustomHeaderModule.md",
            "classes/Platform.Components.Utils/DateUtils.md",
            "MANIFEST.md",
        ]);
        assert!(issues.is_empty(), "{issues:#?}");
    }

    #[test]
    fn three_colliding_spellings_are_still_one_finding_naming_all_of_them() {
        let issues = findings(&[
            "notes/ALPHA.md",
            "notes/Alpha.md",
            "notes/alpha.md",
            "notes/beta.md",
        ]);

        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert_eq!(
            issues[0].message,
            "path `notes/ALPHA.md` differs only in case from `notes/Alpha.md`, `notes/alpha.md`; no macOS or Windows checkout can hold them all, so all but one disappear there without warning",
            "the wording has to stay true when the group is larger than a pair"
        );
    }
}
