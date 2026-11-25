//! Artifact scan, source-root resolution and the desired-set projection.

use std::path::{Path, PathBuf};

use crystalline_core::config::DomainEntry;
use crystalline_core::manifest::ArtifactType;
use crystalline_core::provision::{
    ArtifactListing, ArtifactListingReport, artifact_key, list_artifacts, mcp_key,
    parse_artifact_key,
};
use crystalline_core::{ArtifactFile, resolve_source_roots, scan_domain};

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

// Unix only: the colon fixtures below are not creatable on a Windows file
// system, which closes this hostile-name class off at the OS level there.
#[cfg(unix)]
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

/// **Every name a provisioning notice interpolates is rendered in backticks.**
///
/// The MCP `provision` surface filters notices that name a domain the caller
/// may not see, and it recognizes the name by its backticks: a notice reading
/// "the `lab` domain is virtual" is matched on "`lab`" rather than on the bare
/// word, so an unrelated notice that happens to contain the word is not
/// swallowed and a notice naming the domain in prose is not missed. That filter
/// is a promise about how these strings are written, made in another crate, and
/// nothing here was keeping it true.
///
/// So it is checked where the strings are: any `{name}` or `{domain}` that
/// reaches a message is wrapped. The exceptions are the lines that build a path
/// or a JSON body out of the same variables, which are not prose and are never
/// read by the filter.
#[test]
fn every_name_a_notice_interpolates_is_backticked() {
    const SOURCES: [(&str, &str); 4] = [
        (
            "provision/mod.rs",
            include_str!("../../src/provision/mod.rs"),
        ),
        (
            "provision/model.rs",
            include_str!("../../src/provision/model.rs"),
        ),
        (
            "provision/reconcile.rs",
            include_str!("../../src/provision/reconcile.rs"),
        ),
        (
            "provision/translate.rs",
            include_str!("../../src/provision/translate.rs"),
        ),
    ];
    // Not prose: a path segment, a struct field holding one, a JSON body, and
    // the recorded calls of the test doubles at the bottom of these files.
    let structural = |line: &str| {
        line.contains("out.insert(")
            || line.contains("rel: format!")
            || line.contains("PathBuf::from")
            || line.contains("server_json")
            || line.contains("self.calls.push(")
    };
    for (file, source) in SOURCES {
        // Everything below the test module is fixtures rather than notices.
        let source = source.split("\n#[cfg(test)]\n").next().unwrap_or(source);
        for (n, line) in source.lines().enumerate() {
            if structural(line) {
                continue;
            }
            for placeholder in ["{name}", "{domain}"] {
                if !line.contains(placeholder) {
                    continue;
                }
                assert!(
                    line.contains(&format!("`{placeholder}`")),
                    "{file}:{} interpolates {placeholder} without backticks, so the \
                     MCP notice filter cannot recognize the name it carries:\n{line}",
                    n + 1
                );
            }
        }
    }
}

// --- artifact keys -------------------------------------------------------------

fn artifact(kind: ArtifactType, rel: &str) -> ArtifactFile {
    ArtifactFile {
        kind,
        rel: rel.to_string(),
        source: PathBuf::from(rel),
        sha256: String::new(),
        optional: false,
    }
}

#[test]
fn artifact_keys_follow_the_source_names() {
    // Every file of a skill shares the skill folder's key.
    assert_eq!(
        artifact_key(&artifact(ArtifactType::Skills, "tide-tables/SKILL.md")),
        "skills/tide-tables"
    );
    assert_eq!(
        artifact_key(&artifact(
            ArtifactType::Skills,
            "tide-tables/scripts/chart.sh"
        )),
        "skills/tide-tables"
    );
    // A nested command keeps its namespace slash and drops `.md`.
    assert_eq!(
        artifact_key(&artifact(ArtifactType::Commands, "charts/plot-route.md")),
        "commands/charts/plot-route"
    );
    assert_eq!(
        artifact_key(&artifact(ArtifactType::Commands, "deploy.md")),
        "commands/deploy"
    );
    // An agent is its stem in either dialect.
    assert_eq!(
        artifact_key(&artifact(ArtifactType::Agents, "quartermaster.md")),
        "agents/quartermaster"
    );
    assert_eq!(
        artifact_key(&artifact(ArtifactType::Agents, "quartermaster.toml")),
        "agents/quartermaster"
    );
    // An MCP is the name the scan read from its JSON.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "mcps/beacon.json",
        r#"{"name": "lighthouse", "server": {"type": "http"}}"#,
    );
    let (artifacts, _) = scan_domain("harbor", &[(ArtifactType::Mcps, dir.path().join("mcps"))]);
    assert_eq!(mcp_key(&artifacts.mcps[0]), "mcps/lighthouse");
}

#[test]
fn parse_artifact_key_accepts_the_four_kinds_and_refuses_malformed_ones() {
    assert_eq!(
        parse_artifact_key("skills/tide-tables"),
        Some((ArtifactType::Skills, "tide-tables"))
    );
    assert_eq!(
        parse_artifact_key("commands/deploy"),
        Some((ArtifactType::Commands, "deploy"))
    );
    assert_eq!(
        parse_artifact_key("agents/quartermaster"),
        Some((ArtifactType::Agents, "quartermaster"))
    );
    assert_eq!(
        parse_artifact_key("mcps/lighthouse"),
        Some((ArtifactType::Mcps, "lighthouse"))
    );
    // A nested command name is accepted.
    assert_eq!(
        parse_artifact_key("commands/charts/plot-route"),
        Some((ArtifactType::Commands, "charts/plot-route"))
    );

    for malformed in [
        "skills",                // no slash
        "tools/hammer",          // unknown kind
        "Skills/tide-tables",    // keys compare exactly: the kind is case-sensitive
        "skills/",               // empty name
        "skills/..",             // not a plain component
        "agents/.hidden",        // not a plain component
        "skills/tide/tables",    // a nested skill name
        "mcps/a/b",              // a nested mcp name
        "commands/charts/",      // an empty command segment
        "commands/charts/../up", // `..` inside a command name
        "",
    ] {
        assert_eq!(parse_artifact_key(malformed), None, "{malformed}");
    }
}

// --- the optional mark -----------------------------------------------------------

const OPTIONAL_FRONTMATTER: &str = "---\nname: x\nmetadata:\n  provision: optional\n---\n\nBody.\n";

#[test]
fn a_skill_marked_optional_marks_every_file_of_the_skill() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "skills/storm/SKILL.md", OPTIONAL_FRONTMATTER);
    write(dir.path(), "skills/storm/scripts/warn.sh", "echo storm\n");
    write(dir.path(), "skills/storm/notes/read.md", "No mark here.\n");
    write(
        dir.path(),
        "skills/tide/SKILL.md",
        "---\nname: tide\n---\n\nTides.\n",
    );

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Skills, dir.path().join("skills"))],
    );
    assert!(notices.is_empty(), "{notices:?}");
    let marks: Vec<(&str, bool)> = artifacts
        .files
        .iter()
        .map(|f| (f.rel.as_str(), f.optional))
        .collect();
    assert_eq!(
        marks,
        vec![
            ("storm/SKILL.md", true),
            ("storm/notes/read.md", true),
            ("storm/scripts/warn.sh", true),
            ("tide/SKILL.md", false),
        ]
    );
}

#[test]
fn commands_agents_and_mcps_read_the_optional_mark() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "commands/charts/plot-route.md",
        OPTIONAL_FRONTMATTER,
    );
    write(
        dir.path(),
        "commands/deploy.md",
        "---\ndescription: Deploy\n---\n\nGo.\n",
    );
    write(dir.path(), "agents/quartermaster.md", OPTIONAL_FRONTMATTER);
    write(dir.path(), "agents/scout.md", "Scout ahead.\n");
    write(
        dir.path(),
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "metadata": {"provision": "optional"}, "server": {"type": "http"}}"#,
    );
    write(
        dir.path(),
        "mcps/buoy.json",
        r#"{"name": "buoy", "server": {"type": "stdio"}}"#,
    );

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[
            (ArtifactType::Commands, dir.path().join("commands")),
            (ArtifactType::Agents, dir.path().join("agents")),
            (ArtifactType::Mcps, dir.path().join("mcps")),
        ],
    );
    assert!(notices.is_empty(), "{notices:?}");
    let files: Vec<(String, bool)> = artifacts
        .files
        .iter()
        .map(|f| (artifact_key(f), f.optional))
        .collect();
    assert_eq!(
        files,
        vec![
            ("commands/charts/plot-route".to_string(), true),
            ("commands/deploy".to_string(), false),
            ("agents/quartermaster".to_string(), true),
            ("agents/scout".to_string(), false),
        ]
    );
    let mcps: Vec<(String, bool)> = artifacts
        .mcps
        .iter()
        .map(|m| (mcp_key(m), m.optional))
        .collect();
    assert_eq!(
        mcps,
        vec![
            ("mcps/buoy".to_string(), false),
            ("mcps/lighthouse".to_string(), true),
        ]
    );
}

/// Codex refuses an agent TOML with a `metadata` table, so a `.toml` agent
/// carries no mark of its own: the scan never reads `[metadata]` there. A
/// `.toml` agent beside a markdown agent of the same stem takes the
/// markdown agent's mark, so both files of the one key agree.
#[test]
fn a_toml_agent_carries_no_mark_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let toml = "name = \"reviewer\"\ndescription = \"Reviews\"\ndeveloper_instructions = '''\nGo.\n'''\n\n[metadata]\nprovision = \"optional\"\n";
    write(dir.path(), "agents/reviewer.toml", toml);
    write(dir.path(), "agents/pilot.md", OPTIONAL_FRONTMATTER);
    write(
        dir.path(),
        "agents/pilot.toml",
        "name = \"pilot\"\ndescription = \"Steers\"\ndeveloper_instructions = '''\nSteer.\n'''\n",
    );

    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Agents, dir.path().join("agents"))],
    );
    assert!(notices.is_empty(), "{notices:?}");
    let marks: Vec<(&str, bool)> = artifacts
        .files
        .iter()
        .map(|f| (f.rel.as_str(), f.optional))
        .collect();
    assert_eq!(
        marks,
        vec![
            ("pilot.md", true),
            ("pilot.toml", true),
            ("reviewer.toml", false),
        ]
    );
}

#[test]
fn an_unknown_metadata_provision_value_raises_one_notice_and_reads_as_required() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "skills/storm/SKILL.md",
        "---\nname: storm\nmetadata:\n  provision: maybe\n---\n\nStorms.\n",
    );
    write(dir.path(), "skills/storm/scripts/warn.sh", "echo storm\n");
    write(
        dir.path(),
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "metadata": {"provision": true}, "server": {"type": "http"}}"#,
    );

    let roots = [
        (ArtifactType::Skills, dir.path().join("skills")),
        (ArtifactType::Mcps, dir.path().join("mcps")),
    ];
    let (artifacts, notices) = scan_domain("harbor", &roots);
    assert!(artifacts.files.iter().all(|f| !f.optional));
    assert!(!artifacts.mcps[0].optional);
    // One notice per artifact, not one per file of the skill.
    assert_eq!(
        notices,
        vec![
            "`skills/storm` says `metadata.provision: maybe`; only `optional` has a meaning - treating it as not optional".to_string(),
            "`mcps/lighthouse` says `metadata.provision: true`; only `optional` has a meaning - treating it as not optional".to_string(),
        ]
    );
    // A different case is a different value: only the exact string marks.
    write(
        dir.path(),
        "skills/storm/SKILL.md",
        "---\nname: storm\nmetadata:\n  provision: Optional\n---\n\nStorms.\n",
    );
    let (artifacts, notices) = scan_domain("harbor", &roots[..1]);
    assert!(artifacts.files.iter().all(|f| !f.optional));
    assert_eq!(notices.len(), 1, "{notices:?}");
}

#[test]
fn a_frontmatter_that_does_not_parse_reads_as_not_optional_without_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "commands/broken.md",
        "---\nmetadata: [unterminated\n---\n\nBody.\n",
    );
    let (artifacts, notices) = scan_domain(
        "harbor",
        &[(ArtifactType::Commands, dir.path().join("commands"))],
    );
    assert_eq!(artifacts.files.len(), 1);
    assert!(!artifacts.files[0].optional);
    assert!(notices.is_empty(), "{notices:?}");
}

// --- the light listing -----------------------------------------------------------

/// A harbor with all four kinds, optional and required artifacts of each, a
/// skill with several files and an agent in both dialects.
fn write_marked_harbor(dir: &Path) {
    write_manifest(
        dir,
        "- skills: skills\n- commands: commands\n- agents: agents\n- mcps: mcps\n",
    );
    write(dir, "skills/storm/SKILL.md", OPTIONAL_FRONTMATTER);
    write(dir, "skills/storm/scripts/warn.sh", "echo storm\n");
    write(
        dir,
        "skills/tide/SKILL.md",
        "---\nname: tide\n---\n\nTides.\n",
    );
    write(dir, "skills/tide/notes.md", "Notes.\n");
    write(dir, "commands/charts/plot-route.md", OPTIONAL_FRONTMATTER);
    write(dir, "commands/deploy.md", "Deploy.\n");
    write(dir, "agents/pilot.md", OPTIONAL_FRONTMATTER);
    write(
        dir,
        "agents/pilot.toml",
        "name = \"pilot\"\ndescription = \"Steers\"\ndeveloper_instructions = '''\nSteer.\n'''\n",
    );
    write(
        dir,
        "agents/reviewer.toml",
        "name = \"reviewer\"\ndescription = \"Reviews\"\ndeveloper_instructions = '''\nGo.\n'''\n",
    );
    write(dir, "agents/scout.md", "Scout ahead.\n");
    write(
        dir,
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "metadata": {"provision": "optional"}, "server": {"type": "http"}}"#,
    );
    write(
        dir,
        "mcps/buoy.json",
        r#"{"name": "buoy", "server": {"type": "stdio"}}"#,
    );
    // Skipped by both, with the same notices.
    write(dir, "skills/no-skill/notes.txt", "stray\n");
    write(dir, "mcps/broken.json", "{not json");
    write(
        dir,
        "skills/odd/SKILL.md",
        "---\nmetadata:\n  provision: sometimes\n---\n\nOdd.\n",
    );
}

#[test]
fn list_artifacts_agrees_with_scan_domain_on_keys_and_marks() {
    let dir = tempfile::tempdir().unwrap();
    write_marked_harbor(dir.path());
    let entry = DomainEntry::file(dir.path());

    let (artifacts, scan_notices) = scan_domain("harbor", &resolve_source_roots("harbor", &entry));
    let mut from_scan: Vec<(String, bool)> = artifacts
        .files
        .iter()
        .map(|f| (artifact_key(f), f.optional))
        .chain(artifacts.mcps.iter().map(|m| (mcp_key(m), m.optional)))
        .collect();
    // A skill's files and an agent's two dialects share one key each.
    from_scan.dedup();

    let listing = list_artifacts("harbor", &entry);
    let from_listing: Vec<(String, bool)> = listing
        .artifacts
        .iter()
        .map(|row| (row.key.clone(), row.optional))
        .collect();

    assert_eq!(from_listing, from_scan);
    assert_eq!(
        from_listing,
        vec![
            ("skills/odd".to_string(), false),
            ("skills/storm".to_string(), true),
            ("skills/tide".to_string(), false),
            ("commands/charts/plot-route".to_string(), true),
            ("commands/deploy".to_string(), false),
            ("agents/pilot".to_string(), true),
            ("agents/reviewer".to_string(), false),
            ("agents/scout".to_string(), false),
            ("mcps/buoy".to_string(), false),
            ("mcps/lighthouse".to_string(), true),
        ]
    );
    let mut scan_notices = scan_notices;
    let mut listing_notices = listing.notices.clone();
    scan_notices.sort();
    listing_notices.sort();
    assert_eq!(listing_notices, scan_notices);
    assert_eq!(listing_notices.len(), 3, "{listing_notices:?}");
}

#[test]
fn list_artifacts_lists_a_skill_once() {
    let dir = tempfile::tempdir().unwrap();
    write_marked_harbor(dir.path());
    let listing = list_artifacts("harbor", &DomainEntry::file(dir.path()));

    let storm: Vec<&ArtifactListing> = listing
        .artifacts
        .iter()
        .filter(|row| row.kind == ArtifactType::Skills && row.name == "storm")
        .collect();
    assert_eq!(storm.len(), 1);
    assert_eq!(storm[0].key, "skills/storm");
    assert!(storm[0].optional);

    // A nested command's name keeps its namespace.
    assert!(
        listing
            .artifacts
            .iter()
            .any(|row| row.key == "commands/charts/plot-route" && row.name == "charts/plot-route")
    );
}

#[test]
fn list_artifacts_reports_only_kinds_whose_root_exists() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path(),
        "- skills: skills\n- commands: commands\n- agents: agents\n- mcps: mcps\n",
    );
    write(dir.path(), "skills/tide/SKILL.md", "Tides.\n");
    // An existing but empty agents folder is present; it ships nothing.
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();

    let listing = list_artifacts("harbor", &DomainEntry::file(dir.path()));
    assert_eq!(
        listing.present_kinds,
        vec![ArtifactType::Skills, ArtifactType::Agents]
    );
    assert_eq!(listing.artifacts.len(), 1);
    assert!(listing.notices.is_empty(), "{:?}", listing.notices);
}

#[test]
fn list_artifacts_of_a_virtual_domain_is_empty() {
    let listing = list_artifacts("lab", &DomainEntry::virtual_domain());
    assert_eq!(listing, ArtifactListingReport::default());
}
