//! Artifact scan, source-root resolution and the desired-set projection.

use std::path::Path;

use crystalline_core::config::DomainEntry;
use crystalline_core::manifest::ArtifactType;
use crystalline_core::{resolve_source_roots, scan_domain};

/// Write a harbor-shaped MANIFEST at `dir`, declaring a `## Provisioning`
/// section from `bullets` (already `- `-prefixed lines, one per artifact
/// type).
fn write_manifest(dir: &Path, bullets: &str) {
    let source = format!(
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\n---\n\n\
         # harbor\n\n\
         ## Scope\n\n- Coastal navigation knowledge\n\n\
         ## When to Use\n\n- When docking\n\n\
         ## Provisioning\n\n{bullets}\n"
    );
    std::fs::write(dir.join("MANIFEST.md"), source).unwrap();
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Build the harbor domain fixture from the M4 brief: a `tide-tables` skill
/// (`SKILL.md` plus `scripts/chart.sh`), a `charts/plot-route.md` command, a
/// `quartermaster.md` agent and a `lighthouse.json` mcp.
fn write_harbor(dir: &Path) {
    write_manifest(
        dir,
        "- skills: skills\n- commands: commands\n- agents: agents\n- mcps: mcps\n",
    );
    write(
        dir,
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n",
    );
    write(
        dir,
        "skills/tide-tables/scripts/chart.sh",
        "#!/bin/sh\necho charting\n",
    );
    write(
        dir,
        "commands/charts/plot-route.md",
        "Plot a route between two buoys.\n",
    );
    write(
        dir,
        "agents/quartermaster.md",
        "# Quartermaster\n\nKeeps the manifest of stores.\n",
    );
    // The server keys are deliberately NOT alphabetical (`url` before
    // `type`), so the canonicalization assertion below actually proves the
    // serialized form reorders them - a fixture already in sorted order
    // could not catch a key-order regression.
    write(
        dir,
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "description": "guides ships", "server": {"url": "https://example.test/mcp", "type": "http"}}"#,
    );
}

// --- scan_domain -------------------------------------------------------------

#[test]
fn full_harbor_scan_collects_expected_keys_in_deterministic_order_with_correct_hashes() {
    let dir = tempfile::tempdir().unwrap();
    write_harbor(dir.path());
    let entry = DomainEntry::file(dir.path());
    let roots = resolve_source_roots("harbor", &entry);
    assert_eq!(roots.len(), 4);

    let (artifacts, notices) = scan_domain("harbor", &roots);
    assert!(
        notices.is_empty(),
        "no notices for a clean scan: {notices:?}"
    );
    assert_eq!(artifacts.domain, "harbor");

    let rels: Vec<(ArtifactType, &str)> = artifacts
        .files
        .iter()
        .map(|f| (f.kind, f.rel.as_str()))
        .collect();
    assert_eq!(
        rels,
        vec![
            (ArtifactType::Skills, "tide-tables/SKILL.md"),
            (ArtifactType::Skills, "tide-tables/scripts/chart.sh"),
            (ArtifactType::Commands, "charts/plot-route.md"),
            (ArtifactType::Agents, "quartermaster.md"),
        ]
    );

    // Hand-computed vector: sha256 of the agent file's exact bytes.
    let agent = artifacts
        .files
        .iter()
        .find(|f| f.rel == "quartermaster.md")
        .unwrap();
    assert_eq!(
        agent.sha256,
        "d13e6d6730eb1ef8a687c7d56a8f501a78c4573bac968ab7c24e7787ffdb29b6"
    );

    assert_eq!(artifacts.mcps.len(), 1);
    let lighthouse = &artifacts.mcps[0];
    assert_eq!(lighthouse.name, "lighthouse");
    assert_eq!(
        lighthouse.server_json,
        r#"{"type":"http","url":"https://example.test/mcp"}"#
    );
    assert_eq!(
        lighthouse.sha256,
        "22c6608290f92b95a27276ac36d57368f78a8ff6bab23fa139027deb0edcb7cb"
    );
}

#[test]
fn skill_dir_without_skill_md_is_skipped_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "skills/tide-tables/SKILL.md", "tides\n");
    write(
        dir.path(),
        "skills/no-skill-here/notes.txt",
        "stray notes\n",
    );

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Skills, dir.path().join("skills"))],
    );
    assert_eq!(artifacts.files.len(), 1);
    assert_eq!(artifacts.files[0].rel, "tide-tables/SKILL.md");
    assert!(
        notices
            .iter()
            .any(|n| n.contains("no-skill-here") && n.contains("SKILL.md")),
        "{notices:?}"
    );
}

#[test]
fn hidden_files_are_skipped_silently() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "skills/tide-tables/SKILL.md", "tides\n");
    write(dir.path(), "skills/tide-tables/.DS_Store", "junk\n");
    std::fs::create_dir_all(dir.path().join("skills/tide-tables/.git")).unwrap();
    write(dir.path(), "skills/tide-tables/.git/config", "[core]\n");

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Skills, dir.path().join("skills"))],
    );
    assert_eq!(artifacts.files.len(), 1, "{:?}", artifacts.files);
    assert_eq!(artifacts.files[0].rel, "tide-tables/SKILL.md");
    assert!(
        notices.is_empty(),
        "hidden files produce no notice: {notices:?}"
    );
}

#[test]
fn hostile_names_are_skipped_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    // A colon is a legal filesystem byte on this platform but fails
    // `is_plain_component`, since it is one of the Windows drive-prefix
    // escapes the same check exists to close off. A literal `..` directory
    // cannot be created at all - `std::fs::read_dir` never yields it as an
    // entry - so the colon form is the only one a scan can actually observe
    // on disk; `..` itself is covered directly by `is_plain_component`'s own
    // unit tests instead.
    write(dir.path(), "skills/tide-tables/SKILL.md", "tides\n");
    write(dir.path(), "skills/tide-tables/a:b", "hostile file\n");
    write(dir.path(), "skills/pier:master/SKILL.md", "hostile dir\n");

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Skills, dir.path().join("skills"))],
    );
    let rels: Vec<&str> = artifacts.files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, vec!["tide-tables/SKILL.md"]);
    assert!(
        notices.iter().any(|n| n.contains("a:b")),
        "hostile file notice: {notices:?}"
    );
    assert!(
        notices.iter().any(|n| n.contains("pier:master")),
        "hostile dir notice: {notices:?}"
    );
}

#[test]
fn mcp_without_server_is_skipped_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "mcps/broken.json", r#"{"name": "broken"}"#);

    let (artifacts, notices) =
        scan_domain("harbor", &[(ArtifactType::Mcps, dir.path().join("mcps"))]);
    assert!(artifacts.mcps.is_empty());
    assert!(
        notices
            .iter()
            .any(|n| n.contains("broken.json") && n.contains("server")),
        "{notices:?}"
    );
}

#[test]
fn mcp_name_falls_back_to_the_file_stem_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "mcps/buoy.json",
        r#"{"server": {"type": "stdio", "command": "buoy"}}"#,
    );

    let (artifacts, notices) =
        scan_domain("harbor", &[(ArtifactType::Mcps, dir.path().join("mcps"))]);
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(artifacts.mcps.len(), 1);
    assert_eq!(artifacts.mcps[0].name, "buoy");
}

#[test]
fn a_missing_source_root_contributes_nothing_and_no_notice() {
    let dir = tempfile::tempdir().unwrap();
    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Skills, dir.path().join("nowhere"))],
    );
    assert!(artifacts.files.is_empty());
    assert!(notices.is_empty());
}

/// A symlinked agents entry pointing outside the root must never be
/// followed: following it would let a hostile team repo stage the bytes of
/// any file the link can name (a private key, a token file) for later
/// provisioning. It is skipped silently, the same way `walk_visible`
/// treats a symlink - not a file, not a directory, no notice.
#[cfg(unix)]
#[test]
fn symlinked_agent_outside_the_root_is_skipped_silently() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "outside/secret.md", "not an agent\n");
    write(dir.path(), "agents/quartermaster.md", "real agent\n");
    std::os::unix::fs::symlink(
        dir.path().join("outside/secret.md"),
        dir.path().join("agents/steal.md"),
    )
    .unwrap();

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Agents, dir.path().join("agents"))],
    );
    let rels: Vec<&str> = artifacts.files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, vec!["quartermaster.md"]);
    assert!(notices.is_empty(), "silent skip, no notice: {notices:?}");
}

/// The same no-follow stance for the mcps root: a symlinked `*.json` entry
/// is skipped silently rather than read.
#[cfg(unix)]
#[test]
fn symlinked_mcp_outside_the_root_is_skipped_silently() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "outside/creds.json",
        r#"{"server": {"type": "http", "url": "https://example.test/stolen"}}"#,
    );
    write(
        dir.path(),
        "mcps/buoy.json",
        r#"{"server": {"type": "stdio", "command": "buoy"}}"#,
    );
    std::os::unix::fs::symlink(
        dir.path().join("outside/creds.json"),
        dir.path().join("mcps/steal.json"),
    )
    .unwrap();

    let (artifacts, notices) =
        scan_domain("harbor", &[(ArtifactType::Mcps, dir.path().join("mcps"))]);
    let names: Vec<&str> = artifacts.mcps.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, vec!["buoy"]);
    assert!(notices.is_empty(), "silent skip, no notice: {notices:?}");
}

// --- resolve_source_roots ------------------------------------------------

#[test]
fn local_domain_resolves_in_root_and_escaping_decls() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path(),
        "- skills: skills\n- commands: ../shared-commands\n",
    );
    let entry = DomainEntry::file(dir.path());

    let roots = resolve_source_roots("harbor", &entry);
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0], (ArtifactType::Skills, dir.path().join("skills")));
    assert_eq!(
        roots[1],
        (
            ArtifactType::Commands,
            dir.path().parent().unwrap().join("shared-commands")
        )
    );
}

#[test]
fn team_domain_escaping_decl_resolves_under_the_origin_artifact_mirror() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), "- commands: ../shared-commands\n");
    let mut entry = DomainEntry::file(dir.path());
    entry.origin = Some(crystalline_core::config::OriginConfig {
        repo: "example/harbor".to_string(),
        path: None,
        branch: None,
        poll_secs: None,
    });

    let roots = resolve_source_roots("harbor-team", &entry);
    assert_eq!(roots.len(), 1);
    let expected = crystalline_core::config::origin_state_dir("harbor-team")
        .unwrap()
        .join("artifacts")
        .join("commands");
    assert_eq!(roots[0], (ArtifactType::Commands, expected));
}

#[test]
fn team_domain_in_root_decl_resolves_into_the_working_tree_like_local() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), "- skills: skills\n");
    let mut entry = DomainEntry::file(dir.path());
    entry.origin = Some(crystalline_core::config::OriginConfig {
        repo: "example/harbor".to_string(),
        path: None,
        branch: None,
        poll_secs: None,
    });

    let roots = resolve_source_roots("harbor-team", &entry);
    assert_eq!(
        roots,
        vec![(ArtifactType::Skills, dir.path().join("skills"))]
    );
}

/// A decl that normalizes onto the root itself (`foo/..`) is never a source
/// root - scanning it would sweep the whole domain - for a local domain and
/// a team domain alike, mirroring how `in_root_artifact_dirs` drops it from
/// the exclusion set.
#[test]
fn root_landing_decl_contributes_no_source_root() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path(), "- skills: foo/..\n");

    let local = DomainEntry::file(dir.path());
    assert!(resolve_source_roots("harbor", &local).is_empty());

    let mut team = DomainEntry::file(dir.path());
    team.origin = Some(crystalline_core::config::OriginConfig {
        repo: "example/harbor".to_string(),
        path: None,
        branch: None,
        poll_secs: None,
    });
    assert!(resolve_source_roots("harbor-team", &team).is_empty());
}

#[test]
fn virtual_domain_resolves_to_nothing() {
    let entry = DomainEntry::virtual_domain();
    assert!(resolve_source_roots("notes", &entry).is_empty());
}

#[test]
fn missing_manifest_resolves_to_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let entry = DomainEntry::file(dir.path());
    assert!(resolve_source_roots("harbor", &entry).is_empty());
}
