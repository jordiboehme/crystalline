//! E-family rules: frontmatter format and encoding, plus one rule about the
//! domain's file paths themselves.
//!
//! `E001`-`E006` are unconditional errors: a well-formed Engram must parse,
//! carry the four required fields and use a lowercase slug permalink with
//! clean UTF-8 (`E006`: no byte order mark, no null byte, and valid UTF-8 at
//! all). `E007` (tag format) and the outside-the-recommended-set half
//! of `E003` are softer: Crystalline never enforces a closed `type` or
//! `status` vocabulary, so those two only ever inform. `E008` warns when a
//! permalink starts with the domain's own name: a permalink is
//! domain-relative (the OKF Concept ID made explicit) and the domain name is
//! per-user configuration, so persisting it into a file misleads as soon as
//! the domain is registered under another name. `E010` is the one parse
//! failure with a name of its own: a frontmatter key held more than once,
//! reported per key with its lines when removing the later copies is all the
//! file needs (everything else that does not parse stays `E001`). `E011` warns about a
//! key or value too large for the metadata index.
//! `crystalline doctor --fix` removes the extra copies when they all agree.
//!
//! `E009` is the odd one out twice over: it is about paths rather than
//! frontmatter, and it is domain-scoped rather than per-file, so it runs from
//! [`check_domain`] beside the other domain-level families instead of from
//! [`check`]. Nothing a single document contains can tell you that another
//! document's path collides with it.

use std::collections::BTreeMap;
use std::path::Path;

use crate::address::slugify;
use crate::engram::{
    Frontmatter, META_KEY_MAX_BYTES, META_VALUE_MAX_BYTES, RECOMMENDED_TYPES, meta_value_text,
};
use crate::frontmatter::{DuplicateKey, duplicate_keys, keep_first_copies, line_list};
use crate::parse::{ParseError, parse_engram};

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
        Err(ParseError::NotUtf8 { line }) => {
            sink.emit(
                &file.path,
                Some(*line),
                "E006",
                Severity::Error,
                "file is not valid UTF-8",
                Some("re-save the file as UTF-8 text".into()),
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
            // A key held more than once is its own finding when it is the
            // only thing wrong: with every later copy removed, the file
            // parses. Anything else stays the generic E001. The parse check
            // is a defensive guard: `duplicate_keys` only finds repeats when
            // every block parses alone, so today it always holds.
            let repeated = duplicate_keys(&file.source);
            let only_repeats = !repeated.is_empty()
                && keep_first_copies(&file.source).is_some_and(|text| parse_engram(&text).is_ok());
            if only_repeats {
                // doctor --fix repairs a file only when every repeat in it
                // agrees, so only then does the hint send a person there.
                let doctor_fixes = repeated.iter().all(|r| r.same_value);
                for repeat in &repeated {
                    emit_repeated_key(file, repeat, doctor_fixes, sink);
                }
            } else {
                sink.emit(
                    &file.path,
                    None,
                    "E001",
                    Severity::Error,
                    format!("frontmatter YAML is invalid: {message}"),
                    None,
                );
            }
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

    check_index_limits(&file.path, fm, sink);

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

/// `E011`: a frontmatter key or value the metadata index cannot hold. The
/// index drops a key over [`META_KEY_MAX_BYTES`] and a value whose compact
/// JSON text is over [`META_VALUE_MAX_BYTES`], so a `$contains` filter never
/// finds either. The walk is [`Frontmatter::index_metadata`], so the rule
/// fires for exactly the keys that would get rows, and a value is measured
/// with [`meta_value_text`], byte for byte the way the index does.
pub(crate) fn check_index_limits(path: &Path, fm: &Frontmatter, sink: &mut Sink) {
    let too_long = |value: &serde_json::Value| meta_value_text(value).is_none();
    for (key, value) in &fm.index_metadata() {
        if key.len() > META_KEY_MAX_BYTES {
            sink.emit(
                path,
                None,
                "E011",
                Severity::Warning,
                format!(
                    "frontmatter key `{key}` is longer than {META_KEY_MAX_BYTES} bytes, so the metadata index skips it and a $contains filter cannot find it"
                ),
                Some("use a shorter key".into()),
            );
            continue;
        }
        let over =
            format!("over {META_VALUE_MAX_BYTES} bytes, so a $contains filter cannot find it");
        let fix = Some("shorten the value, or keep long text in the body".to_string());
        match value {
            serde_json::Value::Object(_) => {}
            serde_json::Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if !item.is_array() && !item.is_object() && too_long(item) {
                        sink.emit(
                            path,
                            None,
                            "E011",
                            Severity::Warning,
                            format!("frontmatter key `{key}` element {} is {over}", i + 1),
                            fix.clone(),
                        );
                    }
                }
            }
            scalar if too_long(scalar) => sink.emit(
                path,
                None,
                "E011",
                Severity::Warning,
                format!("frontmatter key `{key}` is {over}"),
                fix,
            ),
            _ => {}
        }
    }
}

/// `E010`: one frontmatter key held more than once, pointing at the second
/// copy's line. `doctor_fixes` says whether `crystalline doctor --fix` would
/// repair the file (every repeat in it agrees); the hint names it only then,
/// in the words doctor itself uses.
fn emit_repeated_key(
    file: &ScannedFile,
    repeat: &DuplicateKey,
    doctor_fixes: bool,
    sink: &mut Sink,
) {
    let count = repeat.lines.len();
    let times = if count == 2 {
        "twice".to_string()
    } else {
        format!("{count} times")
    };
    let (values, fix) = if repeat.same_value {
        let which = if count == 2 { "one" } else { "all but one" };
        let doctor = if doctor_fixes {
            ", or run `crystalline doctor --fix`"
        } else {
            ""
        };
        (
            "the same value",
            format!("delete {which} of the lines{doctor}"),
        )
    } else {
        (
            "different values",
            "keep one line with the right value and delete the rest".to_string(),
        )
    };
    sink.emit(
        &file.path,
        repeat.lines.get(1).copied(),
        "E010",
        Severity::Error,
        format!(
            "frontmatter key `{}` appears {times} (lines {}) with {values}",
            repeat.key,
            line_list(&repeat.lines)
        ),
        Some(fix),
    );
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
/// The folding is [`crate::fold_path_case`], the one both sides call:
/// `crystalline_remote::changes::detect_local_changes` asks the same question
/// of the same paths on the sharing side, and a rule that folded differently
/// from the detector would either report a pair the detector happily folds or
/// stay quiet about one it refuses to.
pub(crate) fn check_domain(domain: &Domain, sink: &mut Sink) {
    // `domain.files` is sorted by path, so each group's paths come out in a
    // stable order and the reported message does not depend on walk order.
    let mut folded: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, file) in domain.files.iter().enumerate() {
        folded
            .entry(crate::fold_path_case(&file.rel_path.to_string_lossy()))
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
            config_problems: Vec::new(),
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

    fn document_findings(source: &str) -> Vec<Issue> {
        crate::verify::check_document("eng", Path::new("alpha.md"), source)
    }

    #[test]
    fn a_key_held_twice_with_one_value_is_one_e010_naming_key_and_lines() {
        let source = "---\ntype: engram\ntitle: Alpha\ndomain_name: eng\npermalink: alpha\nstatus: stable\ndomain_name: eng\n---\n\nBody.\n";
        let issues = document_findings(source);
        assert!(issues.iter().all(|i| i.rule != "E001"), "{issues:#?}");
        let e010: Vec<&Issue> = issues.iter().filter(|i| i.rule == "E010").collect();
        assert_eq!(e010.len(), 1, "{issues:#?}");
        assert_eq!(e010[0].severity, Severity::Error);
        assert_eq!(e010[0].line, Some(7));
        assert_eq!(
            e010[0].message,
            "frontmatter key `domain_name` appears twice (lines 4 and 7) with the same value"
        );
        assert_eq!(
            e010[0].fix.as_deref(),
            Some("delete one of the lines, or run `crystalline doctor --fix`")
        );
    }

    #[test]
    fn a_key_held_three_times_with_different_values_says_so() {
        let source = "---\ntype: engram\ntags: [a]\ntitle: Alpha\ntags: [b]\npermalink: alpha\ntags: [a]\n---\n\nBody.\n";
        let issues = document_findings(source);
        let e010: Vec<&Issue> = issues.iter().filter(|i| i.rule == "E010").collect();
        assert_eq!(e010.len(), 1, "{issues:#?}");
        assert_eq!(
            e010[0].message,
            "frontmatter key `tags` appears 3 times (lines 3, 5 and 7) with different values"
        );
        assert_eq!(
            e010[0].fix.as_deref(),
            Some("keep one line with the right value and delete the rest")
        );
    }

    #[test]
    fn a_repeat_with_one_value_beside_one_with_two_does_not_offer_doctor() {
        let source = "---\ntype: engram\ntags: [a]\ntitle: Alpha\ntags: [b]\nstatus: stable\nstatus: stable\n---\n\nBody.\n";
        let issues = document_findings(source);
        let fixes: Vec<(&str, Option<&str>)> = issues
            .iter()
            .filter(|i| i.rule == "E010")
            .map(|i| (i.message.as_str(), i.fix.as_deref()))
            .collect();
        assert_eq!(
            fixes,
            [
                (
                    "frontmatter key `tags` appears twice (lines 3 and 5) with different values",
                    Some("keep one line with the right value and delete the rest")
                ),
                (
                    "frontmatter key `status` appears twice (lines 6 and 7) with the same value",
                    Some("delete one of the lines")
                ),
            ],
            "{issues:#?}"
        );
    }

    #[test]
    fn a_key_held_three_times_with_one_value_asks_to_delete_all_but_one() {
        let source = "---\ntype: engram\ntitle: Alpha\nstatus: stable\nstatus: stable\nstatus: stable\n---\n\nBody.\n";
        let issues = document_findings(source);
        let e010: Vec<&Issue> = issues.iter().filter(|i| i.rule == "E010").collect();
        assert_eq!(e010.len(), 1, "{issues:#?}");
        assert_eq!(
            e010[0].fix.as_deref(),
            Some("delete all but one of the lines, or run `crystalline doctor --fix`")
        );
    }

    #[test]
    fn a_repeat_beside_another_yaml_error_stays_e001() {
        let source = "---\ntype: engram\ntitle: \"Alpha\nstatus: a\nstatus: a\n---\n\nBody.\n";
        let rules: Vec<&str> = document_findings(source).iter().map(|i| i.rule).collect();
        assert_eq!(rules, ["E001"]);
    }

    fn with_extra(line: &str) -> String {
        format!(
            "---\ntype: engram\ntitle: Alpha\ndomain_name: eng\npermalink: alpha\nstatus: stable\n{line}\n---\n\nBody.\n"
        )
    }

    fn e011(source: &str) -> Vec<Issue> {
        document_findings(source)
            .into_iter()
            .filter(|i| i.rule == "E011")
            .collect()
    }

    #[test]
    fn a_key_over_256_bytes_is_one_e011_naming_it() {
        let key = "k".repeat(257);
        let found = e011(&with_extra(&format!("{key}: x")));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].severity, Severity::Warning);
        assert!(found[0].message.contains(&format!("`{key}`")));
        assert!(found[0].message.contains("longer than 256 bytes"));
    }

    #[test]
    fn a_custom_value_over_1024_bytes_is_one_e011() {
        let found = e011(&with_extra(&format!("note: {}", "v".repeat(1100))));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].message.contains("`note`"));
        assert!(
            found[0]
                .message
                .contains("over 1024 bytes, so a $contains filter cannot find it")
        );
        assert_eq!(
            found[0].fix.as_deref(),
            Some("shorten the value, or keep long text in the body")
        );
    }

    #[test]
    fn one_long_list_element_is_one_e011_naming_its_position() {
        let source = with_extra(&format!(
            "refs:\n  - short\n  - {}\n  - tiny",
            "v".repeat(1100)
        ));
        let found = e011(&source);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].message.contains("`refs` element 2 "), "{found:#?}");
    }

    #[test]
    fn a_promoted_key_over_1kb_gets_no_e011() {
        let source = format!(
            "---\ntype: engram\ntitle: {}\ndomain_name: eng\npermalink: alpha\nstatus: stable\n---\n\nBody.\n",
            "t".repeat(1500)
        );
        assert!(e011(&source).is_empty());
    }

    #[test]
    fn a_value_exactly_at_the_limit_gets_nothing() {
        // The compact JSON text carries two quote marks.
        let value = "v".repeat(META_VALUE_MAX_BYTES - 2);
        assert!(e011(&with_extra(&format!("note: {value}"))).is_empty());
        let key = "k".repeat(META_KEY_MAX_BYTES);
        assert!(e011(&with_extra(&format!("{key}: x"))).is_empty());
        let over = "v".repeat(META_VALUE_MAX_BYTES - 1);
        assert_eq!(e011(&with_extra(&format!("note: {over}"))).len(), 1);
    }

    #[test]
    fn e011_can_be_turned_off_in_the_domain_config() {
        let found = e011(&with_extra(&format!("note: {}", "v".repeat(1100))));
        let mut cfg = crate::config::VerifyConfig::default();
        cfg.rules.insert("E011".to_string(), "off".to_string());
        assert!(crate::verify::apply_overrides(found.clone(), Some(&cfg)).is_empty());
        assert_eq!(crate::verify::apply_overrides(found, None).len(), 1);
    }
}
