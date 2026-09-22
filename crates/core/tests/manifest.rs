//! Manifest section extraction and routing bullets.

mod common;

use common::{fixtures_dir, read};
use crystalline_core::manifest::{
    ArtifactType, GeneratedIndexes, Manifest, PolicyKey, PolicyRole, ProblemKind, SHARING_KEY,
    Sharing, TagAliasProblemKind, append_tag_alias, generated_indexes_at, in_root_artifact_dirs,
    policy_registry, sharing_at, tag_alias_pairs,
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

// --- generated_indexes: the frontmatter switch ------------------------------

/// A MANIFEST source declaring `generated_indexes` as `declared`, or nothing
/// at all when `declared` is `None`.
fn manifest_declaring(declared: Option<&str>) -> String {
    let line = match declared {
        Some(value) => format!("generated_indexes: {value}\n"),
        None => String::new(),
    };
    format!(
        "---\ntype: manifest\ntitle: KB\n{line}---\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n"
    )
}

#[test]
fn a_manifest_declaring_nothing_keeps_its_generated_indexes_local() {
    // The default, and the one every MANIFEST written before the switch
    // existed lands on.
    let m = manifest_from_source(&manifest_declaring(None));
    assert_eq!(m.declared_generated_indexes(), None);
    assert_eq!(m.generated_indexes(), GeneratedIndexes::Local);
    assert_eq!(GeneratedIndexes::default(), GeneratedIndexes::Local);
}

#[test]
fn a_manifest_declaring_shared_lets_its_generated_indexes_travel() {
    let m = manifest_from_source(&manifest_declaring(Some("shared")));
    assert_eq!(m.declared_generated_indexes(), Some("shared"));
    assert_eq!(m.generated_indexes(), GeneratedIndexes::Shared);
}

#[test]
fn a_manifest_declaring_local_says_so_explicitly() {
    let m = manifest_from_source(&manifest_declaring(Some("local")));
    assert_eq!(m.declared_generated_indexes(), Some("local"));
    assert_eq!(m.generated_indexes(), GeneratedIndexes::Local);
}

#[test]
fn a_generated_indexes_value_nobody_recognizes_is_never_read_as_shared() {
    // The whole point of the two words: a typo, a boolean somebody reached for
    // out of habit, or a value in the wrong case must never quietly start
    // publishing files. Every one of these reads as `local` and is reported by
    // verify rule `M006`; the declaration is still readable verbatim so the
    // finding can quote it back.
    for value in ["Shared", "SHARED", "true", "yes", "sharde", "null", "42"] {
        let m = manifest_from_source(&manifest_declaring(Some(value)));
        assert_eq!(
            m.generated_indexes(),
            GeneratedIndexes::Local,
            "`{value}` must not be read as shared"
        );
        assert!(
            GeneratedIndexes::parse(m.declared_generated_indexes().expect("declared")).is_none(),
            "`{value}` must not parse as a policy"
        );
    }

    // And a value that is not a scalar at all: named by its shape so the
    // finding can still say what it found.
    let listed = manifest_from_source(&manifest_declaring(Some("\n  - shared")));
    assert_eq!(listed.declared_generated_indexes(), Some("a list"));
    assert_eq!(listed.generated_indexes(), GeneratedIndexes::Local);
}

#[test]
fn generated_indexes_spellings_round_trip() {
    assert_eq!(GeneratedIndexes::Local.as_str(), "local");
    assert_eq!(GeneratedIndexes::Shared.as_str(), "shared");
    assert_eq!(
        GeneratedIndexes::parse("local"),
        Some(GeneratedIndexes::Local)
    );
    assert_eq!(
        GeneratedIndexes::parse("shared"),
        Some(GeneratedIndexes::Shared)
    );
    assert_eq!(GeneratedIndexes::parse("elsewhere"), None);
}

#[test]
fn generated_indexes_at_reads_the_domain_root_and_falls_back_to_local() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // No MANIFEST at all.
    assert_eq!(generated_indexes_at(root), GeneratedIndexes::Local);

    // An unparseable one.
    std::fs::write(root.join("MANIFEST.md"), "---\nnot: [valid\n").unwrap();
    assert_eq!(generated_indexes_at(root), GeneratedIndexes::Local);

    // One that declares nothing.
    std::fs::write(root.join("MANIFEST.md"), manifest_declaring(None)).unwrap();
    assert_eq!(generated_indexes_at(root), GeneratedIndexes::Local);

    // And one that declares the listings travel.
    std::fs::write(root.join("MANIFEST.md"), manifest_declaring(Some("shared"))).unwrap();
    assert_eq!(generated_indexes_at(root), GeneratedIndexes::Shared);
}

// --- sharing: the frontmatter switch ----------------------------------------

/// A MANIFEST declaring `sharing: <value>`, or nothing, beside the two
/// required sections.
fn manifest_declaring_sharing(declared: Option<&str>) -> String {
    let line = match declared {
        Some(value) => format!("sharing: {value}\n"),
        None => String::new(),
    };
    format!(
        "---\ntype: manifest\ntitle: KB\n{line}---\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n"
    )
}

#[test]
fn a_manifest_declaring_nothing_shares_as_a_proposal() {
    let m = manifest_from_source(&manifest_declaring_sharing(None));
    assert_eq!(m.declared_sharing(), None);
    assert_eq!(m.sharing(), Sharing::Proposal);
    assert_eq!(Sharing::default(), Sharing::Proposal);
}

#[test]
fn a_manifest_declaring_direct_commits_straight_to_the_branch() {
    let m = manifest_from_source(&manifest_declaring_sharing(Some("direct")));
    assert_eq!(m.declared_sharing(), Some("direct"));
    assert_eq!(m.sharing(), Sharing::Direct);
}

#[test]
fn a_sharing_value_nobody_recognizes_is_never_read_as_direct() {
    // The safe side is the reviewed one: a typo, the wrong case or a boolean
    // must never turn a review step off. Every one of these reads as
    // `proposal` and is reported by verify rule `M007`; the declaration is
    // still readable verbatim so the finding can quote it back.
    for value in ["Direct", "DIRECT", "dirct", "true", "yes", "null", "42"] {
        let m = manifest_from_source(&manifest_declaring_sharing(Some(value)));
        assert_eq!(
            m.sharing(),
            Sharing::Proposal,
            "`{value}` must not be read as direct"
        );
        assert_eq!(m.declared_sharing(), Some(value));
        assert!(
            Sharing::parse(value).is_none(),
            "`{value}` must not parse as a policy"
        );
    }
    let listed = manifest_from_source(&manifest_declaring_sharing(Some("\n  - direct")));
    assert_eq!(listed.declared_sharing(), Some("a list"));
    assert_eq!(listed.sharing(), Sharing::Proposal);
}

#[test]
fn sharing_spellings_round_trip() {
    assert_eq!(Sharing::Proposal.as_str(), "proposal");
    assert_eq!(Sharing::Direct.as_str(), "direct");
    assert_eq!(Sharing::parse("proposal"), Some(Sharing::Proposal));
    assert_eq!(Sharing::parse("direct"), Some(Sharing::Direct));
    assert_eq!(Sharing::parse("review"), None);
}

#[test]
fn sharing_at_reads_the_domain_root_and_falls_back_to_proposal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(sharing_at(root), Sharing::Proposal, "no MANIFEST at all");
    std::fs::write(root.join("MANIFEST.md"), "---\nnot: [valid\n").unwrap();
    assert_eq!(sharing_at(root), Sharing::Proposal, "an unparseable one");
    std::fs::write(root.join("MANIFEST.md"), manifest_declaring_sharing(None)).unwrap();
    assert_eq!(
        sharing_at(root),
        Sharing::Proposal,
        "one that declares nothing"
    );
    std::fs::write(
        root.join("MANIFEST.md"),
        manifest_declaring_sharing(Some("direct")),
    )
    .unwrap();
    assert_eq!(sharing_at(root), Sharing::Direct);
}

// --- The policy registry ----------------------------------------------------

/// Every key the registry names answers through `Manifest::policy`, its
/// default is one of its own values and an undeclared MANIFEST reads as that
/// default. A key added to the registry without an accessor fails here.
#[test]
fn every_registry_key_answers_through_manifest_policy() {
    let silent = manifest_from_source(&manifest_declaring_sharing(None));
    assert_eq!(policy_registry().len(), 2, "generated_indexes and sharing");
    for spec in policy_registry() {
        assert!(
            spec.values.contains(&spec.default),
            "{}: default is a value",
            spec.key
        );
        assert!(
            !spec.meaning.is_empty() && !spec.meaning.contains('\n'),
            "{}: one line",
            spec.key
        );
        let (declared, effective) = silent
            .policy(spec.key)
            .unwrap_or_else(|| panic!("`{}` has no accessor behind it", spec.key));
        assert_eq!(declared, None, "{}", spec.key);
        assert_eq!(effective, spec.default, "{}", spec.key);
    }
    assert_eq!(
        silent.policy("colour"),
        None,
        "a key the registry does not know"
    );
    let direct = manifest_from_source(&manifest_declaring_sharing(Some("direct")));
    assert_eq!(direct.policy(SHARING_KEY), Some((Some("direct"), "direct")));
    let typo = manifest_from_source(&manifest_declaring_sharing(Some("dirct")));
    assert_eq!(typo.policy(SHARING_KEY), Some((Some("dirct"), "proposal")));
}

/// The two rows, as the card and the doctor read them.
#[test]
fn the_registry_rows_say_who_changes_what_and_to_which_values() {
    let by_key = |key: &str| -> PolicyKey {
        *policy_registry()
            .iter()
            .find(|spec| spec.key == key)
            .unwrap_or_else(|| panic!("{key} is registered"))
    };
    let indexes = by_key("generated_indexes");
    assert_eq!(indexes.values, &["local", "shared"]);
    assert_eq!(indexes.default, "local");
    assert_eq!(indexes.changed_by, PolicyRole::Owner);
    let sharing = by_key(SHARING_KEY);
    assert_eq!(sharing.values, &["proposal", "direct"]);
    assert_eq!(sharing.default, "proposal");
    assert_eq!(sharing.changed_by, PolicyRole::Owner);
    assert_eq!(PolicyRole::Owner.as_str(), "owner");
    assert_eq!(PolicyRole::Admin.as_str(), "admin");
}

/// Every `pub const *_KEY` the parser declares is in the registry, and every
/// registry key is such a constant: the guard that makes "a key without a
/// registry entry fails" true at the source rather than by review. The shape
/// of the collation guard in crates/index.
#[test]
fn every_manifest_key_constant_is_in_the_registry_and_back() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/manifest.rs"))
        .expect("the parser's own source");
    let declared: Vec<String> = source
        .lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("pub const ")?;
            let (name, value) = rest.split_once(": &str = \"")?;
            name.ends_with("_KEY")
                .then(|| value.split('"').next().unwrap_or_default().to_string())
        })
        .collect();
    assert!(!declared.is_empty(), "the scan found the key constants");
    let registered: Vec<&str> = policy_registry().iter().map(|spec| spec.key).collect();
    for key in &declared {
        assert!(
            registered.contains(&key.as_str()),
            "`{key}` is declared and not registered"
        );
    }
    for key in &registered {
        assert!(
            declared.iter().any(|d| d == key),
            "`{key}` is registered and not declared"
        );
    }
}
