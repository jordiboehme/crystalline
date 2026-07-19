//! Manifest section extraction and routing bullets.

mod common;

use common::{fixtures_dir, read};
use crystalline_core::manifest::{
    ArtifactType, Manifest, ProblemKind, TagAliasProblemKind, append_tag_alias,
    in_root_artifact_dirs, tag_alias_pairs,
};
use crystalline_core::parse_engram;

fn manifest(rel: &str) -> (Manifest, String) {
    let source = read(&fixtures_dir().join(rel));
    let engram = parse_engram(&source).unwrap();
    (Manifest::from_engram(&engram, &source), source)
}

/// Build a Manifest straight from an inline MANIFEST source string, for the
/// `## Provisioning` tests below where a fixture file would be overkill.
fn manifest_from_source(source: &str) -> Manifest {
    let engram = parse_engram(source).unwrap();
    Manifest::from_engram(&engram, source)
}

#[test]
fn valid_manifest_has_required_sections() {
    let (m, _) = manifest("manifests/manifest-valid.md");
    assert!(m.has_scope());
    assert!(m.has_when_to_use());
    assert!(m.missing_required_sections().is_empty());
    assert_eq!(m.scope().len(), 3);
    assert_eq!(m.when_to_use().len(), 3);
}

#[test]
fn routing_bullets_prefer_when_to_use() {
    let (m, _) = manifest("manifests/manifest-valid.md");
    let routing = m.routing_bullets();
    assert_eq!(routing, m.when_to_use());
    assert!(routing[0].starts_with("When a question is about growing"));
}

#[test]
fn invalid_manifest_missing_when_to_use() {
    let (m, _) = manifest("manifests/manifest-invalid.md");
    assert!(m.has_scope());
    assert!(!m.has_when_to_use());
    assert_eq!(m.missing_required_sections(), ["When to Use"]);
    // Routing falls back to Scope.
    assert_eq!(m.routing_bullets(), m.scope());
    assert_eq!(m.scope().len(), 2);
}

#[test]
fn section_matching_is_case_insensitive_and_first_wins() {
    let source = "\
---
type: manifest
title: MANIFEST
---

# KB

## scope

- first scope bullet

## Scope

- duplicate scope bullet that loses

## When To Use

- routing one
";
    let engram = parse_engram(source).unwrap();
    let m = Manifest::from_engram(&engram, source);
    assert!(m.has_scope());
    assert!(m.has_when_to_use());
    // First duplicate wins: only the earlier Scope bullet is kept.
    assert_eq!(m.scope(), ["first scope bullet"]);
    assert_eq!(m.when_to_use(), ["routing one"]);
}

// --- Provisioning ------------------------------------------------------------

#[test]
fn provisioning_absent_is_none() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Scope

- some scope
";
    let m = manifest_from_source(source);
    assert!(m.provisioning().is_none());
}

#[test]
fn four_valid_decls_parse_in_order() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills: ../skills
- commands: ../commands
- agents: ../agents
- mcps: mcp
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert!(section.problems.is_empty());
    assert_eq!(section.decls.len(), 4);
    assert_eq!(section.decls[0].kind, ArtifactType::Skills);
    assert_eq!(section.decls[0].path, "../skills");
    assert_eq!(section.decls[1].kind, ArtifactType::Commands);
    assert_eq!(section.decls[1].path, "../commands");
    assert_eq!(section.decls[2].kind, ArtifactType::Agents);
    assert_eq!(section.decls[2].path, "../agents");
    assert_eq!(section.decls[3].kind, ArtifactType::Mcps);
    assert_eq!(section.decls[3].path, "mcp");
}

#[test]
fn unknown_type_is_a_problem_not_an_error() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- prompts: ../prompts
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert!(section.decls.is_empty());
    assert_eq!(section.problems.len(), 1);
    assert_eq!(section.problems[0].kind, ProblemKind::UnknownType);
    assert_eq!(section.problems[0].bullet, "prompts: ../prompts");
    assert!(section.problems[0].reason.contains("prompts"));
}

#[test]
fn missing_colon_and_empty_path_are_problems() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills without a colon
- commands:
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert!(section.decls.is_empty());
    assert_eq!(section.problems.len(), 2);
    assert_eq!(section.problems[0].kind, ProblemKind::Malformed);
    assert_eq!(section.problems[0].bullet, "skills without a colon");
    assert_eq!(section.problems[1].kind, ProblemKind::Malformed);
    assert_eq!(section.problems[1].bullet, "commands:");
}

#[test]
fn absolute_and_tilde_paths_are_problems() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills: /abs
- commands: \\abs
- agents: ~/x
- mcps: nested/C:evil
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert!(section.decls.is_empty());
    assert_eq!(section.problems.len(), 4);
    assert!(
        section
            .problems
            .iter()
            .all(|p| p.kind == ProblemKind::InvalidPath)
    );
}

#[test]
fn duplicate_type_first_wins() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills: ../skills
- skills: ../other-skills
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert_eq!(section.decls.len(), 1);
    assert_eq!(section.decls[0].path, "../skills");
    assert_eq!(section.problems.len(), 1);
    assert_eq!(section.problems[0].kind, ProblemKind::DuplicateType);
    assert_eq!(section.problems[0].bullet, "skills: ../other-skills");
}

#[test]
fn parent_segments_are_accepted_at_parse_time() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills: ../skills
- commands: ../../nested/skills
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert!(section.problems.is_empty());
    assert_eq!(section.decls.len(), 2);
    assert_eq!(section.decls[0].path, "../skills");
    assert_eq!(section.decls[1].path, "../../nested/skills");
}

#[test]
fn trailing_slash_is_trimmed() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills: ../skills/
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert_eq!(section.decls.len(), 1);
    assert_eq!(section.decls[0].path, "../skills");
}

#[test]
fn empty_provisioning_section_has_no_decls_or_problems() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

## Scope

- some scope
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert!(section.decls.is_empty());
    assert!(section.problems.is_empty());
}

#[test]
fn provisioning_heading_is_case_insensitive() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## provisioning

- skills: ../skills
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert_eq!(section.decls.len(), 1);
    assert_eq!(section.decls[0].kind, ArtifactType::Skills);
}

#[test]
fn fenced_bullets_inside_provisioning_are_ignored() {
    let source = "\
---
type: manifest
title: KB
---

# KB

## Provisioning

- skills: ../skills

```text
- skills: x
```
";
    let m = manifest_from_source(source);
    let section = m.provisioning().expect("section present");
    assert_eq!(section.decls.len(), 1);
    assert_eq!(section.decls[0].path, "../skills");
}

// --- in_root_artifact_dirs ---------------------------------------------------

/// Write a harbor MANIFEST with the given body into `dir`, so the
/// `in_root_artifact_dirs` tests can point the helper at a real file on disk.
fn write_manifest(dir: &std::path::Path, body: &str) {
    let source = format!(
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\n---\n\n# harbor\n\n{body}"
    );
    std::fs::write(dir.join("MANIFEST.md"), source).unwrap();
}

#[test]
fn in_root_decl_is_returned() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), "## Provisioning\n\n- skills: skills\n");
    assert_eq!(
        in_root_artifact_dirs(dir.path()),
        [dir.path().join("skills")]
    );
}

#[test]
fn out_of_root_decl_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), "## Provisioning\n\n- skills: ../skills\n");
    assert!(in_root_artifact_dirs(dir.path()).is_empty());
}

#[test]
fn root_landing_decl_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    // `foo/..` normalizes onto the root itself; excluding it would exclude the
    // whole domain, so the helper drops it.
    write_manifest(dir.path(), "## Provisioning\n\n- skills: foo/..\n");
    assert!(in_root_artifact_dirs(dir.path()).is_empty());
}

#[test]
fn dot_segments_are_normalized_before_joining() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), "## Provisioning\n\n- skills: a/../skills\n");
    assert_eq!(
        in_root_artifact_dirs(dir.path()),
        [dir.path().join("skills")]
    );
}

#[test]
fn missing_manifest_yields_no_dirs() {
    let dir = tempfile::tempdir().unwrap();
    assert!(in_root_artifact_dirs(dir.path()).is_empty());
}

#[test]
fn garbage_manifest_yields_no_dirs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("MANIFEST.md"),
        "just some prose, no frontmatter",
    )
    .unwrap();
    assert!(in_root_artifact_dirs(dir.path()).is_empty());
}

// --- Tag Aliases -------------------------------------------------------------

/// Wrap a `## Tag Aliases` body into a minimal MANIFEST source.
fn manifest_with_tag_aliases(aliases: &str) -> String {
    format!("---\ntype: manifest\ntitle: KB\n---\n\n# KB\n\n## Tag Aliases\n\n{aliases}")
}

#[test]
fn tag_aliases_absent_is_none() {
    let source = "---\ntype: manifest\ntitle: KB\n---\n\n## Scope\n\n- some scope\n";
    assert!(manifest_from_source(source).tag_aliases().is_none());
}

#[test]
fn tag_aliases_happy_path_parses_decls() {
    // The alias side is never linted: `multi_word` with an underscore is kept
    // verbatim on purpose, since recording the old name is the point.
    let source = manifest_with_tag_aliases("- multi_word -> multi-word\n- oldName -> new-name\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    assert!(section.problems.is_empty(), "{:#?}", section.problems);
    assert_eq!(section.decls.len(), 2);
    assert_eq!(section.decls[0].alias, "multi_word");
    assert_eq!(section.decls[0].canonical, "multi-word");
    assert_eq!(section.decls[1].alias, "oldName");
    assert_eq!(section.decls[1].canonical, "new-name");
}

#[test]
fn tag_aliases_malformed_bullets_are_problems() {
    let source = manifest_with_tag_aliases("- no arrow here\n- -> missing-alias\n- lonely ->\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    assert!(section.decls.is_empty());
    assert_eq!(section.problems.len(), 3);
    assert!(
        section
            .problems
            .iter()
            .all(|p| p.kind == TagAliasProblemKind::Malformed)
    );
}

#[test]
fn tag_aliases_self_alias_is_a_problem_with_no_decl() {
    // Fold-equal sides: `Foo` and `foo` are the same tag, nothing to merge.
    let source = manifest_with_tag_aliases("- Foo -> foo\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    assert!(section.decls.is_empty());
    assert_eq!(section.problems.len(), 1);
    assert_eq!(section.problems[0].kind, TagAliasProblemKind::SelfAlias);
}

#[test]
fn tag_aliases_exact_duplicate_is_silently_dropped() {
    let source = manifest_with_tag_aliases("- foo -> bar\n- foo -> bar\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    assert_eq!(section.decls.len(), 1);
    assert!(section.problems.is_empty(), "{:#?}", section.problems);
}

#[test]
fn tag_aliases_conflicting_alias_first_wins() {
    let source = manifest_with_tag_aliases("- foo -> bar\n- foo -> baz\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    assert_eq!(section.decls.len(), 1);
    assert_eq!(section.decls[0].canonical, "bar");
    assert_eq!(section.problems.len(), 1);
    assert_eq!(
        section.problems[0].kind,
        TagAliasProblemKind::DuplicateAlias
    );
    assert_eq!(section.problems[0].bullet, "foo -> baz");
}

#[test]
fn tag_aliases_chained_target_is_kept_and_flagged() {
    // `a -> b` chains onto `b -> c`; both decls stay, only `a -> b` is flagged,
    // and resolution never collapses to `a -> c`.
    let source = manifest_with_tag_aliases("- a -> b\n- b -> c\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    assert_eq!(section.decls.len(), 2);
    assert_eq!(section.problems.len(), 1);
    assert_eq!(section.problems[0].kind, TagAliasProblemKind::ChainedAlias);
    assert_eq!(section.problems[0].bullet, "a -> b");
}

#[test]
fn tag_aliases_non_canonical_target_is_kept_and_flagged() {
    let source = manifest_with_tag_aliases("- old -> Not_Canonical\n");
    let section = manifest_from_source(&source)
        .tag_aliases()
        .expect("section present");
    // The decl is kept - the index folds the target anyway - but flagged.
    assert_eq!(section.decls.len(), 1);
    assert_eq!(section.decls[0].canonical, "Not_Canonical");
    assert_eq!(section.problems.len(), 1);
    assert_eq!(
        section.problems[0].kind,
        TagAliasProblemKind::NonCanonicalTarget
    );
}

#[test]
fn tag_alias_pairs_folds_and_skips_problems() {
    let source = manifest_with_tag_aliases(
        "- Multi_Word -> Multi-Word\n- bad line without arrow\n- dup -> one\n- dup -> two\n",
    );
    // `Multi_Word -> Multi-Word` is a kept (non-canonical) decl and folds in;
    // the arrowless bullet and the losing duplicate are skipped.
    assert_eq!(
        tag_alias_pairs(&source),
        vec![
            ("multi_word".to_string(), "multi-word".to_string()),
            ("dup".to_string(), "one".to_string()),
        ]
    );
}

#[test]
fn tag_alias_pairs_work_without_frontmatter_and_are_empty_otherwise() {
    // parse_engram tolerates a missing frontmatter block, so a body-only
    // MANIFEST still yields its folded pairs.
    let source = "## Tag Aliases\n\n- Old_Name -> new-name\n";
    assert_eq!(
        tag_alias_pairs(source),
        [("old_name".to_string(), "new-name".to_string())]
    );
    // No section at all: empty.
    assert!(tag_alias_pairs("plain prose, no sections here").is_empty());
}

#[test]
fn append_tag_alias_creates_section_at_eof() {
    let src = "---\ntype: manifest\ntitle: KB\n---\n\n# KB\n\n## Scope\n\n- something\n";
    let out = append_tag_alias(src, "old", "new").expect("appended");
    assert_eq!(
        out,
        "---\ntype: manifest\ntitle: KB\n---\n\n# KB\n\n## Scope\n\n- something\n\n## Tag Aliases\n\n- old -> new\n"
    );
}

#[test]
fn append_tag_alias_creates_section_at_eof_without_trailing_newline() {
    // No final newline: the helper inserts one before the new block.
    let src = "---\ntype: manifest\ntitle: KB\n---\n\n# KB\n\n## Scope\n\n- something";
    let out = append_tag_alias(src, "old", "new").expect("appended");
    assert_eq!(
        out,
        "---\ntype: manifest\ntitle: KB\n---\n\n# KB\n\n## Scope\n\n- something\n\n## Tag Aliases\n\n- old -> new\n"
    );
}

#[test]
fn append_tag_alias_after_last_bullet() {
    let src = "---\ntype: manifest\ntitle: KB\n---\n\n## Tag Aliases\n\n- a -> b\n- c -> d\n";
    let out = append_tag_alias(src, "e", "f").expect("appended");
    assert_eq!(
        out,
        "---\ntype: manifest\ntitle: KB\n---\n\n## Tag Aliases\n\n- a -> b\n- c -> d\n- e -> f\n"
    );
}

#[test]
fn append_tag_alias_into_empty_section() {
    // An empty section: the bullet lands after the heading and its blank line.
    let src = "---\ntype: manifest\ntitle: KB\n---\n\n## Tag Aliases\n\n## Scope\n\n- s\n";
    let out = append_tag_alias(src, "old", "new").expect("appended");
    assert_eq!(
        out,
        "---\ntype: manifest\ntitle: KB\n---\n\n## Tag Aliases\n\n- old -> new\n## Scope\n\n- s\n"
    );
}

#[test]
fn append_tag_alias_is_byte_preserving_over_a_non_canonical_manifest() {
    // Unusual spacing, a non-canonical existing alias and a following section:
    // only the one new bullet line is spliced in, everything else is verbatim.
    let src = "---\ntype: manifest\ntitle:   Weird Spacing\npermalink: manifest\n---\n\n#  KB\n\n## Tag Aliases\n\n- old_one -> new-one\n\n## Notes\n\n- keep me\n";
    let out = append_tag_alias(src, "old_two", "new-two").expect("appended");
    let expected = "---\ntype: manifest\ntitle:   Weird Spacing\npermalink: manifest\n---\n\n#  KB\n\n## Tag Aliases\n\n- old_one -> new-one\n- old_two -> new-two\n\n## Notes\n\n- keep me\n";
    assert_eq!(out, expected);
}

#[test]
fn append_tag_alias_returns_none_on_existing_pair_case_folded() {
    let src = "---\ntype: manifest\ntitle: KB\n---\n\n## Tag Aliases\n\n- foo -> bar\n";
    // The pair is present when folded, so a differently cased request is a no-op.
    assert!(append_tag_alias(src, "Foo", "BAR").is_none());
}

#[test]
fn append_tag_alias_ignores_a_fenced_fake_heading() {
    // A `## Tag Aliases` inside a code fence must not be treated as the section:
    // a fresh section is appended at EOF and the fence survives byte-for-byte.
    let src = "---\ntype: manifest\ntitle: KB\n---\n\n## Scope\n\n- s\n\n```\n## Tag Aliases\n\n- fake -> fake-canonical\n```\n";
    let out = append_tag_alias(src, "old", "new").expect("appended");
    let expected = "---\ntype: manifest\ntitle: KB\n---\n\n## Scope\n\n- s\n\n```\n## Tag Aliases\n\n- fake -> fake-canonical\n```\n\n## Tag Aliases\n\n- old -> new\n";
    assert_eq!(out, expected);
}

#[test]
fn append_tag_alias_preserves_crlf_endings_and_appends_lf() {
    // Pin the current behavior on a CRLF-line-ending MANIFEST: every original
    // byte is kept verbatim (CRLF endings intact) and the appended bullet uses
    // the LF ending the helper always produces. No EOL detection is attempted.
    let src = "---\r\ntype: manifest\r\ntitle: KB\r\n---\r\n\r\n## Tag Aliases\r\n\r\n- a -> b\r\n";
    let out = append_tag_alias(src, "c", "d").expect("appended");
    assert_eq!(
        out,
        format!("{src}- c -> d\n"),
        "every CRLF byte survives and only an LF-terminated bullet is spliced in"
    );
    assert!(
        tag_alias_pairs(&out).contains(&("c".to_string(), "d".to_string())),
        "the appended pair parses from the CRLF source: {out:?}"
    );
}
