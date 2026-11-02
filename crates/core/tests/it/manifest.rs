//! Manifest section extraction and routing bullets.

use crate::common::{fixtures_dir, read};
use crystalline_core::manifest::{
    ArtifactType, DOMAIN_NAME_KEY, EVOLVE_HOME_KEY, GeneratedIndexes, Manifest, PolicyKey,
    PolicyKind, PolicyRole, ProblemKind, READ_ONLY_KEY, REFLECTION_HOME_KEY, REFLECTION_KEY,
    ReadOnly, ReflectionSwitch, SHARING_KEY, Sharing, TEAM_KEY, TagAliasProblemKind, TeamForge,
    append_tag_alias, domain_name_at, domain_name_of_source, evolve_home_at, generated_indexes_at,
    in_root_artifact_dirs, manifest_template, policy_registry, read_only_at, read_only_of_source,
    reflection_at, reflection_home_at, set_team_marker, sharing_at, starter_stanzas,
    tag_alias_pairs, team_at, team_of_source, validate_reflection_home,
};
use crystalline_core::manifest_view::{RoutingSection, manifest_facts, policy_rows, section_rows};
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

// --- The domain_name declaration ---------------------------------------------

fn manifest_declaring_domain_name(value: Option<&str>) -> String {
    let line = value
        .map(|v| format!("domain_name: {v}\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: manifest\ntitle: t\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n{line}---\n\n# t\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n"
    )
}

#[test]
fn domain_name_is_read_only_when_it_is_a_valid_string() {
    let ok = manifest_from_source(&manifest_declaring_domain_name(Some("platform")));
    assert_eq!(ok.domain_name(), Some("platform"));
    assert_eq!(
        ok.policy(DOMAIN_NAME_KEY),
        Some((Some("platform"), "platform"))
    );
    for bad in ["../up", "1.0", "true", "123", "null", "'a b'", "[a, b]"] {
        let m = manifest_from_source(&manifest_declaring_domain_name(Some(bad)));
        assert_eq!(m.domain_name(), None, "{bad}");
        assert!(m.declared_domain_name().is_some(), "{bad}");
    }
    let quoted = manifest_from_source(&manifest_declaring_domain_name(Some("'123'")));
    assert_eq!(quoted.domain_name(), Some("123"));
    let silent = manifest_from_source(&manifest_declaring_domain_name(None));
    assert_eq!(silent.policy(DOMAIN_NAME_KEY), Some((None, "")));
}

#[test]
fn domain_name_at_reads_the_root_and_falls_back_to_none() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(domain_name_at(dir.path()), None);
    std::fs::write(
        dir.path().join("MANIFEST.md"),
        manifest_declaring_domain_name(Some("eng")),
    )
    .unwrap();
    assert_eq!(domain_name_at(dir.path()).as_deref(), Some("eng"));
}

#[test]
fn the_declared_name_goes_right_after_the_title() {
    let source =
        "---\ntype: manifest\ntitle: KB\npermalink: manifest\nstatus: stable\n---\n\n# KB\n";
    let named = crystalline_core::manifest::set_declared_name(source, "kb");
    assert_eq!(
        named,
        "---\ntype: manifest\ntitle: KB\ndomain_name: kb\npermalink: manifest\nstatus: stable\n---\n\n# KB\n"
    );
    assert_eq!(domain_name_of_source(&named).as_deref(), Some("kb"));
}

#[test]
fn a_name_can_be_declared_unless_the_frontmatter_cannot_be_cut_into_keys() {
    use crystalline_core::manifest::can_declare_name;
    assert!(can_declare_name("---\ntitle: KB\n---\n"));
    assert!(can_declare_name(
        "---\ntitle: KB\n\"domain_name\": kb\n---\n"
    ));
    assert!(
        can_declare_name("# KB\n"),
        "no frontmatter: a block is created"
    );
    assert!(!can_declare_name(
        "---\n{title: KB, domain_name: kb}\n---\n"
    ));
}

#[test]
fn the_template_declares_the_name_right_after_the_title() {
    let source = manifest_template("eng", "2026-09-28");
    assert!(
        source.contains("title: eng\ndomain_name: eng\npermalink: manifest\n"),
        "{source}"
    );
}

#[test]
fn the_template_declares_a_valid_name_so_it_reads_back_as_the_same_string() {
    for name in ["eng", "1.0", "true", "123", "a.b"] {
        let source = manifest_template(name, "2026-09-25");
        assert_eq!(
            domain_name_of_source(&source).as_deref(),
            Some(name),
            "{source}"
        );
    }
    let source = manifest_template("My Notes", "2026-09-25");
    assert!(
        !source.contains("domain_name"),
        "an invalid name is not declared: {source}"
    );
}

// --- The policy registry ----------------------------------------------------

/// Every key the registry names answers through `Manifest::policy`, its
/// default is one of its own values and an undeclared MANIFEST reads as that
/// default. A key added to the registry without an accessor fails here.
#[test]
fn every_registry_key_answers_through_manifest_policy() {
    let silent = manifest_from_source(&manifest_declaring_sharing(None));
    assert_eq!(
        policy_registry().len(),
        10,
        "generated_indexes, sharing, domain_name, read_only, reflection, reflection_home, reflect_trigger, evolve_trigger, team and evolve_home"
    );
    for spec in policy_registry() {
        if spec.kind == PolicyKind::Choice {
            assert!(
                spec.values.contains(&spec.default),
                "{}: default is a value",
                spec.key
            );
        } else {
            assert!(
                spec.values.is_empty(),
                "{}: a text key has no values",
                spec.key
            );
            assert_eq!(
                spec.default, "",
                "{}: a text key defaults to empty",
                spec.key
            );
        }
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

/// The first four rows, as the card and the doctor read them.
#[test]
fn the_registry_rows_say_who_changes_what_and_to_which_values() {
    let by_key = |key: &str| -> PolicyKey {
        *policy_registry()
            .iter()
            .find(|spec| spec.key == key)
            .unwrap_or_else(|| panic!("{key} is registered"))
    };
    let indexes = by_key("generated_indexes");
    assert_eq!(indexes.kind, PolicyKind::Choice);
    assert_eq!(indexes.values, &["local", "shared"]);
    assert_eq!(indexes.default, "local");
    assert_eq!(indexes.changed_by, PolicyRole::Owner);
    let sharing = by_key(SHARING_KEY);
    assert_eq!(sharing.kind, PolicyKind::Choice);
    assert_eq!(sharing.values, &["proposal", "direct"]);
    assert_eq!(sharing.default, "proposal");
    assert_eq!(sharing.changed_by, PolicyRole::Owner);
    assert_eq!(PolicyRole::Owner.as_str(), "owner");
    assert_eq!(PolicyRole::Admin.as_str(), "admin");

    let name = by_key(DOMAIN_NAME_KEY);
    assert_eq!(name.kind, PolicyKind::Text);
    assert_eq!(name.changed_by, PolicyRole::Owner);
    assert!(name.accepts("platform"));
    assert!(!name.accepts("../up"));
    assert!(by_key(SHARING_KEY).accepts("direct"));
    assert!(!by_key(SHARING_KEY).accepts("dirct"));

    let locked = by_key(READ_ONLY_KEY);
    assert_eq!(locked.kind, PolicyKind::Choice);
    assert_eq!(locked.values, &["off", "on"]);
    assert_eq!(locked.default, "off");
    assert_eq!(locked.changed_by, PolicyRole::Owner);
    assert!(locked.accepts("on") && locked.accepts("off") && !locked.accepts("true"));
    assert_eq!(
        policy_registry()[3].key,
        READ_ONLY_KEY,
        "appended, so the rows that were there keep their places"
    );
    assert_eq!(
        policy_registry()[7].key,
        "evolve_trigger",
        "the team row is appended after it"
    );
    assert_eq!(
        policy_registry()[5].key,
        REFLECTION_HOME_KEY,
        "the trigger rows are appended after it"
    );
    assert_eq!(
        policy_registry()[9].key,
        EVOLVE_HOME_KEY,
        "appended after the team row"
    );
    let evolve_home = by_key(EVOLVE_HOME_KEY);
    assert_eq!(evolve_home.kind, PolicyKind::Text);
    assert_eq!(evolve_home.changed_by, PolicyRole::Owner);
    assert!(evolve_home.accepts("https://kb.example.com"));
    assert!(!evolve_home.accepts("https://kb.example.com/"));
    assert!(!evolve_home.accepts("somewhere"));
}

/// `evolve_home` reads like `reflection_home`: a server address in the
/// spelling the server writes its own, else absent.
#[test]
fn evolve_home_is_a_url_or_nothing() {
    let home = manifest_from_source(&manifest_declaring_reflection(
        "evolve_home: https://kb.example.com\n",
    ));
    assert_eq!(home.evolve_home(), Some("https://kb.example.com"));
    assert_eq!(
        home.policy(EVOLVE_HOME_KEY),
        Some((Some("https://kb.example.com"), "https://kb.example.com"))
    );
    let junk = manifest_from_source(&manifest_declaring_reflection(
        "evolve_home: https://KB.example.com/\n",
    ));
    assert_eq!(junk.declared_evolve_home(), Some("https://KB.example.com/"));
    assert_eq!(junk.evolve_home(), None);
    assert_eq!(
        junk.policy(EVOLVE_HOME_KEY),
        Some((Some("https://KB.example.com/"), ""))
    );
    let silent = manifest_from_source(&manifest_declaring_reflection(""));
    assert_eq!(silent.evolve_home(), None);
    assert_eq!(silent.policy(EVOLVE_HOME_KEY), Some((None, "")));

    let dir = tempfile::tempdir().unwrap();
    assert_eq!(evolve_home_at(dir.path()), None, "no MANIFEST");
    std::fs::write(
        dir.path().join("MANIFEST.md"),
        manifest_declaring_reflection("evolve_home: http://localhost:8080\n"),
    )
    .unwrap();
    assert_eq!(
        evolve_home_at(dir.path()).as_deref(),
        Some("http://localhost:8080")
    );
}

fn manifest_declaring_reflection(lines: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: KB\n{lines}---\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n"
    )
}

#[test]
fn reflection_is_off_unless_the_manifest_says_on_exactly() {
    let silent = manifest_from_source(&manifest_declaring_reflection(""));
    assert_eq!(silent.reflection(), ReflectionSwitch::Off);
    assert_eq!(silent.policy(REFLECTION_KEY), Some((None, "off")));
    let on = manifest_from_source(&manifest_declaring_reflection("reflection: on\n"));
    assert_eq!(on.reflection(), ReflectionSwitch::On);
    assert_eq!(on.policy(REFLECTION_KEY), Some((Some("on"), "on")));
    // YAML 1.1 would read a bare `on` as a boolean; the frontmatter parser keeps
    // it as text, and a quoted spelling means the same.
    let quoted = manifest_from_source(&manifest_declaring_reflection("reflection: \"on\"\n"));
    assert_eq!(quoted.reflection(), ReflectionSwitch::On);
    let typo = manifest_from_source(&manifest_declaring_reflection("reflection: One\n"));
    assert_eq!(
        typo.reflection(),
        ReflectionSwitch::Off,
        "a typo is the safe side"
    );
    assert_eq!(typo.policy(REFLECTION_KEY), Some((Some("One"), "off")));
}

#[test]
fn reflection_home_is_a_url_or_nothing() {
    let home = manifest_from_source(&manifest_declaring_reflection(
        "reflection: on\nreflection_home: https://kb.example.com\n",
    ));
    assert_eq!(home.reflection_home(), Some("https://kb.example.com"));
    assert_eq!(
        home.policy(REFLECTION_HOME_KEY),
        Some((Some("https://kb.example.com"), "https://kb.example.com"))
    );
    let junk = manifest_from_source(&manifest_declaring_reflection(
        "reflection_home: somewhere\n",
    ));
    assert_eq!(junk.declared_reflection_home(), Some("somewhere"));
    assert_eq!(junk.reflection_home(), None);
    assert_eq!(
        junk.policy(REFLECTION_HOME_KEY),
        Some((Some("somewhere"), ""))
    );

    assert!(validate_reflection_home("https://kb.example.com").is_ok());
    assert!(validate_reflection_home("http://localhost:8080").is_ok());
    assert!(
        validate_reflection_home("https://kb.example.com/").is_err(),
        "no trailing slash"
    );
    assert!(
        validate_reflection_home("https://kb.example.com/kb").is_ok(),
        "a base path"
    );
    assert!(
        validate_reflection_home("https://kb.example.com/kb/").is_err(),
        "no trailing slash"
    );
    assert!(validate_reflection_home("https://kb.example.com/kb?x").is_err());
    assert!(validate_reflection_home("https://kb.example.com//kb").is_err());
    assert!(validate_reflection_home("ftp://kb.example.com").is_err());
    assert!(validate_reflection_home("https://").is_err());
    assert!(validate_reflection_home("https://a b").is_err());
    // Spellings the server's own address never takes, so they would never
    // compare equal with it.
    assert!(
        validate_reflection_home("https://:8080").is_err(),
        "no host"
    );
    assert!(
        validate_reflection_home("https://kb.example.com:443").is_err(),
        "default port"
    );
    assert!(
        validate_reflection_home("http://kb.example.com:80").is_err(),
        "default port"
    );
    assert!(
        validate_reflection_home("https://kb.example.com:8443").is_ok(),
        "another port"
    );
    assert!(
        validate_reflection_home("https://KB.example.com").is_err(),
        "uppercase host"
    );
    assert!(
        validate_reflection_home("HTTPS://kb.example.com").is_err(),
        "uppercase scheme"
    );
    assert!(validate_reflection_home("https://kb.example.com:port").is_err());
    assert!(validate_reflection_home("https://user@kb.example.com").is_err());
    assert!(
        validate_reflection_home(" https://kb.example.com").is_err(),
        "whitespace"
    );
    let err = validate_reflection_home("https://KB.example.com:443").unwrap_err();
    assert!(err.contains("`https://kb.example.com`"), "{err}");
}

#[test]
fn the_two_trigger_rows_and_the_accessor() {
    use crystalline_core::automation::Loop;
    use crystalline_core::{EVOLVE_TRIGGER_KEY, REFLECT_TRIGGER_KEY, Trigger};
    for (key, lp) in [
        (REFLECT_TRIGGER_KEY, Loop::Reflect),
        (EVOLVE_TRIGGER_KEY, Loop::Evolve),
    ] {
        let spec = *policy_registry().iter().find(|s| s.key == key).unwrap();
        assert_eq!(spec.kind, PolicyKind::Choice);
        assert_eq!(spec.values, &["off", "webhook"]);
        assert_eq!(spec.default, "off");
        assert_eq!(spec.changed_by, PolicyRole::Owner);
        let silent = manifest_from_source(&manifest_declaring_sharing(None));
        assert_eq!(silent.trigger(lp), Trigger::Off);
        assert_eq!(silent.policy(key), Some((None, "off")));
        let on = manifest_from_source(&manifest_declaring_reflection(&format!("{key}: webhook\n")));
        assert_eq!(on.trigger(lp), Trigger::Webhook);
        assert_eq!(on.policy(key), Some((Some("webhook"), "webhook")));
        let typo = manifest_from_source(&manifest_declaring_reflection(&format!("{key}: hook\n")));
        assert_eq!(typo.trigger(lp), Trigger::Off, "a typo never sends");
        assert_eq!(typo.policy(key), Some((Some("hook"), "off")));
    }
    let only_reflect =
        manifest_from_source(&manifest_declaring_reflection("reflect_trigger: webhook\n"));
    assert_eq!(only_reflect.trigger(Loop::Evolve), Trigger::Off);
}

#[test]
fn the_two_reflection_rows() {
    let by_key = |key: &str| -> PolicyKey {
        *policy_registry()
            .iter()
            .find(|spec| spec.key == key)
            .unwrap()
    };
    let switch = by_key(REFLECTION_KEY);
    assert_eq!(switch.kind, PolicyKind::Choice);
    assert_eq!(switch.values, &["off", "on"]);
    assert_eq!(switch.default, "off");
    assert_eq!(switch.changed_by, PolicyRole::Owner);
    let home = by_key(REFLECTION_HOME_KEY);
    assert_eq!(home.kind, PolicyKind::Text);
    assert_eq!(home.changed_by, PolicyRole::Owner);
    assert!(home.accepts("https://kb.example.com"));
    assert!(!home.accepts("kb.example.com"));
}

#[test]
fn an_unset_reflection_home_reads_empty_and_the_name_still_falls_back() {
    let manifest = manifest_from_source(&manifest_declaring_reflection(""));
    for rows in [
        crystalline_core::manifest_view::policy_rows(Some(&manifest), "eng"),
        crystalline_core::manifest_view::policy_rows(None, "eng"),
    ] {
        let row = |key: &str| {
            rows.iter()
                .find(|r| r.key == key)
                .unwrap()
                .effective
                .clone()
        };
        assert_eq!(row("reflection_home"), "", "not the domain name");
        assert_eq!(
            row("domain_name"),
            "eng",
            "the one text key with a fallback"
        );
        assert_eq!(row("reflection"), "off");
    }
}

#[test]
fn reflection_at_reads_the_domain_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(reflection_at(root), ReflectionSwitch::Off, "no MANIFEST");
    // A null byte is refused by the format layer, so this MANIFEST does not parse.
    let unparseable = "---\ntype: manifest\nreflection: on\n---\n\0\n";
    assert!(parse_engram(unparseable).is_err());
    std::fs::write(root.join("MANIFEST.md"), unparseable).unwrap();
    assert_eq!(reflection_at(root), ReflectionSwitch::Off, "unparseable");
    std::fs::write(root.join("MANIFEST.md"), manifest_declaring_reflection("")).unwrap();
    assert_eq!(reflection_at(root), ReflectionSwitch::Off, "silent");
    std::fs::write(
        root.join("MANIFEST.md"),
        manifest_declaring_reflection("reflection: on\nreflection_home: https://kb.example.com\n"),
    )
    .unwrap();
    assert_eq!(reflection_at(root), ReflectionSwitch::On);
    assert_eq!(
        reflection_home_at(root).as_deref(),
        Some("https://kb.example.com")
    );
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

fn manifest_declaring_read_only(value: Option<&str>) -> String {
    let line = value
        .map(|v| format!("read_only: {v}\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\n{line}---\n\n## Scope\n\n- Charts of the harbor\n\n## When to Use\n\n- When asked about the harbor\n"
    )
}

/// `on` locks, `off` and an absent key do not, and anything else reads as
/// `off`: the registry's rule for every key. `on` needs no quoting, because the
/// YAML the parser reads is 1.2, where `on` is a string and not a boolean.
#[test]
fn read_only_reads_on_exactly_and_everything_else_as_off() {
    let on = manifest_from_source(&manifest_declaring_read_only(Some("on")));
    assert_eq!(on.read_only(), ReadOnly::On);
    assert_eq!(on.policy(READ_ONLY_KEY), Some((Some("on"), "on")));
    let off = manifest_from_source(&manifest_declaring_read_only(Some("off")));
    assert_eq!(off.read_only(), ReadOnly::Off);
    let absent = manifest_from_source(&manifest_declaring_read_only(None));
    assert_eq!(absent.read_only(), ReadOnly::Off);
    assert_eq!(absent.policy(READ_ONLY_KEY), Some((None, "off")));
    for typo in ["true", "yes", "On", "\"on \""] {
        let m = manifest_from_source(&manifest_declaring_read_only(Some(typo)));
        assert_eq!(m.read_only(), ReadOnly::Off, "`{typo}` is not `on`");
        assert!(
            m.declared_read_only().is_some(),
            "`{typo}` is still reported"
        );
    }
    assert_eq!(
        read_only_of_source(&manifest_declaring_read_only(Some("on"))),
        ReadOnly::On
    );
    assert_eq!(read_only_of_source("no frontmatter at all"), ReadOnly::Off);
    assert!(ReadOnly::On.is_on() && !ReadOnly::Off.is_on());
    assert_eq!(ReadOnly::default(), ReadOnly::Off);
}

/// B-3: `read_only_of_source` skips the YAML parse when the text cannot name
/// the key, and the answer is the full parse's answer either way: a key
/// spelled through an escape still locks, and a source that only mentions
/// the word in its body or fails to parse stays open.
#[test]
fn read_only_of_source_answers_the_full_parse_whichever_path_it_takes() {
    let head = "---\ntype: manifest\ntitle: M\npermalink: manifest\nstatus: current\n";
    let body = "---\n\n## Scope\n\n- Charts\n";
    let cases = [
        (format!("{head}read_only: on\n{body}"), ReadOnly::On),
        (format!("{head}\"read_only\": \"on\"\n{body}"), ReadOnly::On),
        (format!("{head}'read_only': on\n{body}"), ReadOnly::On),
        (format!("{head}\"read\\x5fonly\": on\n{body}"), ReadOnly::On),
        (
            format!("{head}\"read\\u005Fonly\": on\n{body}"),
            ReadOnly::On,
        ),
        (format!("{head}read_only: off\n{body}"), ReadOnly::Off),
        (format!("{head}{body}"), ReadOnly::Off),
        (
            format!("{head}{body}\nThe read_only: on line belongs in the frontmatter.\n"),
            ReadOnly::Off,
        ),
        (format!("{head}read_only: [on\n{body}"), ReadOnly::Off),
        // Windows line endings still lock. A leading byte-order mark and a
        // double-quoted key folded across lines with a backslash-newline
        // (a multi-line implicit key, which YAML refuses) do not: the
        // shortcut answers what the full parse answers for each.
        (
            format!("{head}read_only: on\n{body}").replace('\n', "\r\n"),
            ReadOnly::On,
        ),
        (
            format!("\u{feff}{head}read_only: on\n{body}"),
            ReadOnly::Off,
        ),
        (
            format!("{head}\"read\\\n  _only\": on\n{body}"),
            ReadOnly::Off,
        ),
        (
            format!("{head}\"read\\\n  _only\": on\n{body}").replace('\n', "\r\n"),
            ReadOnly::Off,
        ),
        ("no frontmatter at all".to_string(), ReadOnly::Off),
        ("---\nread_only: on\n".to_string(), ReadOnly::Off),
        (String::new(), ReadOnly::Off),
    ];
    for (source, expected) in &cases {
        assert_eq!(read_only_of_source(source), *expected, "{source:?}");
        let full = match crystalline_core::parse_engram(source) {
            Ok(engram) => Manifest::from_engram(&engram, source).read_only(),
            Err(_) => ReadOnly::Off,
        };
        assert_eq!(full, *expected, "the full parse agrees on {source:?}");
    }
}

#[test]
fn read_only_at_reads_the_folder_manifest_and_defaults_to_off() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_only_at(dir.path()), ReadOnly::Off, "no MANIFEST");
    std::fs::write(
        dir.path().join("MANIFEST.md"),
        manifest_declaring_read_only(Some("on")),
    )
    .unwrap();
    assert_eq!(read_only_at(dir.path()), ReadOnly::On);
    std::fs::write(dir.path().join("MANIFEST.md"), "---\nbroken: [\n---\n").unwrap();
    assert_eq!(read_only_at(dir.path()), ReadOnly::Off, "unparseable");
}

// --- The starter stanzas ----------------------------------------------------

/// **Every starter stanza parses back into the declaration it advertises.**
///
/// The example is shown to a person as a promise about what the parser
/// accepts. Keeping it beside the parser makes a drift a test failure rather
/// than a wrong screenshot.
#[test]
fn every_starter_stanza_parses_into_what_it_describes() {
    for stanza in starter_stanzas() {
        assert!(
            !stanza.meaning.is_empty() && !stanza.meaning.contains('\n'),
            "{}: one line",
            stanza.section
        );
        let source = format!(
            "---\ntype: manifest\ntitle: eng\npermalink: manifest\nstatus: stable\n---\n\n# eng\n\n{}\n",
            stanza.example
        );
        let parsed = manifest_from_source(&source);
        match stanza.section {
            "When to Use" => assert!(
                !parsed.when_to_use().is_empty(),
                "the When to Use example leaves a routing bullet"
            ),
            "Scope" => assert!(
                !parsed.scope().is_empty(),
                "the Scope example leaves a scope bullet"
            ),
            "Provisioning" => {
                let section = parsed
                    .provisioning()
                    .expect("the Provisioning example declares the section");
                assert_eq!(
                    section.decls.len(),
                    1,
                    "the Provisioning example declares exactly one folder"
                );
                assert!(section.problems.is_empty(), "{:?}", section.problems);
            }
            "Tag Aliases" => {
                let section = parsed
                    .tag_aliases()
                    .expect("the Tag Aliases example declares the section");
                assert_eq!(section.decls.len(), 1);
                assert!(section.problems.is_empty(), "{:?}", section.problems);
            }
            other => panic!("no assertion for {other}"),
        }
    }
}

/// The four sections a domain page offers to start, in the order it draws
/// them. A section added to the registry without a place on the page, or the
/// other way round, fails here.
#[test]
fn the_starter_registry_names_every_section_the_page_offers() {
    let sections: Vec<&str> = starter_stanzas()
        .iter()
        .map(|stanza| stanza.section)
        .collect();
    assert_eq!(
        sections,
        ["When to Use", "Scope", "Provisioning", "Tag Aliases"]
    );
    for stanza in starter_stanzas() {
        assert!(
            stanza
                .example
                .starts_with(&format!("## {}\n", stanza.section)),
            "{}: the example opens with the heading it starts",
            stanza.section
        );
    }
}

const EVERY_SECTION: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\ngenerated_indexes: shared\nsharing: direct\ndomain_name: engineering\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n\n## Provisioning\n\n- skills: skills\n- widgets: w\n\n## Tag Aliases\n\n- k8s -> kubernetes\n- Kube -> k8s\n";

#[test]
fn manifest_facts_reads_every_policy_and_section() {
    let facts = manifest_facts(EVERY_SECTION, "eng");
    assert!(facts.parsed);
    let keys: Vec<&str> = facts.policies.iter().map(|p| p.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "generated_indexes",
            "sharing",
            "domain_name",
            "read_only",
            "reflection",
            "reflection_home",
            "reflect_trigger",
            "evolve_trigger",
            "team",
            "evolve_home"
        ]
    );
    assert_eq!(facts.policies[1].declared.as_deref(), Some("direct"));
    assert_eq!(facts.policies[1].effective, "direct");
    assert_eq!(facts.policies[2].kind, "text");
    assert_eq!(facts.routing, RoutingSection::WhenToUse);
    assert!(facts.missing.is_empty());
    let names: Vec<&str> = facts.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["When to Use", "Scope", "Provisioning", "Tag Aliases"]
    );
    assert!(facts.sections.iter().all(|s| s.present));
    let required: Vec<bool> = facts.sections.iter().map(|s| s.required).collect();
    assert_eq!(required, [true, true, false, false]);
    assert!(
        facts
            .provisioning
            .unwrap()
            .problems
            .iter()
            .any(|p| p.kind == "unknown_type")
    );
    assert!(
        facts
            .tag_aliases
            .unwrap()
            .problems
            .iter()
            .any(|p| p.kind == "chained_alias")
    );
}

#[test]
fn a_manifest_that_does_not_parse_declares_nothing() {
    // A null byte is refused by the format layer.
    let facts = manifest_facts("---\ntype: manifest\n---\n\0\n", "eng");
    assert!(!facts.parsed);
    assert!(facts.policies.is_empty());
    assert_eq!(facts.routing, RoutingSection::None);
    assert_eq!(facts.missing, ["Scope", "When to Use"]);
    assert!(facts.sections.iter().all(|s| !s.present));
}

#[test]
fn policy_rows_without_a_manifest_are_the_registry_defaults() {
    let rows = policy_rows(None, "eng");
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[0].effective, "local");
    assert_eq!(rows[1].effective, "proposal");
    assert!(rows.iter().all(|r| r.declared.is_none()));
    assert_eq!(
        rows[2].effective, "eng",
        "free text falls back to the local name"
    );
    assert_eq!(rows[3].effective, "off");
    assert_eq!(section_rows(None).len(), 4);
}

// --- The team marker ----------------------------------------------------------

#[test]
fn the_team_marker_names_its_forge_or_reads_as_absent() {
    let silent = manifest_from_source(&manifest_declaring_reflection(""));
    assert_eq!(silent.declared_team(), None);
    assert_eq!(silent.team(), None);
    assert_eq!(silent.policy(TEAM_KEY), Some((None, "")));
    let github = manifest_from_source(&manifest_declaring_reflection("team: github\n"));
    assert_eq!(github.team(), Some(TeamForge::GitHub));
    assert_eq!(github.policy(TEAM_KEY), Some((Some("github"), "github")));
    for typo in ["GitHub", "gitlab", "yes"] {
        let other =
            manifest_from_source(&manifest_declaring_reflection(&format!("team: {typo}\n")));
        assert_eq!(other.declared_team(), Some(typo));
        assert_eq!(other.team(), None, "{typo} reads as absent");
        assert_eq!(other.policy(TEAM_KEY), Some((Some(typo), "")));
    }
}

#[test]
fn the_team_row_is_text_that_only_the_engine_writes() {
    let spec = *policy_registry()
        .iter()
        .find(|spec| spec.key == TEAM_KEY)
        .expect("team is registered");
    assert_eq!(spec.kind, PolicyKind::Text);
    assert_eq!(spec.default, "");
    assert!(spec.accepts("github"));
    assert!(!spec.accepts("GitHub") && !spec.accepts("gitlab") && !spec.accepts(""));
    assert_eq!(
        policy_registry()[8].key,
        TEAM_KEY,
        "appended, so the rows that were there keep their places"
    );
}

#[test]
fn team_at_and_team_of_source_read_the_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(team_at(root), None, "no MANIFEST");
    std::fs::write(root.join("MANIFEST.md"), manifest_declaring_reflection("")).unwrap();
    assert_eq!(team_at(root), None, "silent");
    let marked = manifest_declaring_reflection("team: github\n");
    std::fs::write(root.join("MANIFEST.md"), &marked).unwrap();
    assert_eq!(team_at(root), Some(TeamForge::GitHub));
    assert_eq!(team_of_source(&marked), Some(TeamForge::GitHub));
    assert_eq!(team_of_source("no frontmatter"), None);
}

#[test]
fn set_team_marker_writes_one_line_after_the_title_and_is_idempotent() {
    let source = manifest_declaring_reflection("reflection: off\n");
    let marked = set_team_marker(&source, TeamForge::GitHub);
    assert!(marked.contains("title: KB\nteam: github\n"), "{marked}");
    assert_eq!(team_of_source(&marked), Some(TeamForge::GitHub));
    assert_eq!(set_team_marker(&marked, TeamForge::GitHub), marked);
    // The rest of the text is kept as it was.
    assert_eq!(marked.replace("team: github\n", ""), source);
}
