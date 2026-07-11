//! Manifest section extraction and routing bullets.

mod common;

use common::{fixtures_dir, read};
use crystalline_core::manifest::{ArtifactType, Manifest, ProblemKind, in_root_artifact_dirs};
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
