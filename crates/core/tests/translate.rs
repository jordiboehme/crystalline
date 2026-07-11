//! The bidirectional translation matrix through the public [`desired_set`]:
//! byte-identical passthrough for every native pair, cross-dialect renders for
//! the rest, and the notices for dropped fields, skipped pairs and flatten
//! collisions. A Claude-authored domain must serve Codex and Copilot users, and
//! a Codex-authored domain must serve Claude and Copilot users.

use std::path::Path;

use crystalline_core::provision::receipt::sha256_hex;
use crystalline_core::{ArtifactType, DesiredPayload, DomainArtifacts, HarnessKind, desired_set};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Scan the four artifact folders under `dir` (missing ones contribute
/// nothing) into a [`DomainArtifacts`] named `domain`.
fn scan(domain: &str, dir: &Path) -> DomainArtifacts {
    let roots = vec![
        (ArtifactType::Skills, dir.join("skills")),
        (ArtifactType::Commands, dir.join("commands")),
        (ArtifactType::Agents, dir.join("agents")),
        (ArtifactType::Mcps, dir.join("mcps")),
    ];
    let (artifacts, _notices) = crystalline_core::scan_domain(domain, &roots);
    artifacts
}

// The Claude-dialect agent body and the Codex-dialect agent body, kept as
// constants so the render assertions can quote them exactly.
const HARBOR_AGENT_MD: &str = "---\nname: quartermaster\ndescription: Keeps the ship's stores\nmodel: opus\ntools:\n  - Read\n---\n\nKeep the stores in order.\n";
const HARBOR_COMMAND: &str =
    "---\ndescription: Plot a route\nargument-hint: <from> <to>\n---\n\nPlot a route.\n";
const COVE_AGENT_TOML: &str = "name = \"reviewer\"\ndescription = \"Reviews code carefully\"\nmodel = \"gpt-5-codex\"\ndeveloper_instructions = '''\nBe thorough.\nName every risk.\n'''\n";
const COVE_PROMPT: &str = "---\ndescription: Deploy the fleet\n---\n\nDeploy everything.\n";

/// A Claude-dialect harbor: a skill, a nested markdown command, a markdown
/// agent and an MCP config.
fn write_harbor(dir: &Path) {
    write(
        dir,
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nTides.\n",
    );
    write(dir, "commands/charts/plot-route.md", HARBOR_COMMAND);
    write(dir, "agents/quartermaster.md", HARBOR_AGENT_MD);
    write(
        dir,
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "server": {"type": "http", "url": "https://example.test/mcp"}}"#,
    );
}

/// A Codex-dialect cove: a flat prompt and a TOML agent.
fn write_cove(dir: &Path) {
    write(dir, "commands/deploy.md", COVE_PROMPT);
    write(dir, "agents/reviewer.toml", COVE_AGENT_TOML);
}

// --- Claude-authored set serves every harness --------------------------------

#[test]
fn claude_dialect_passes_through_to_claude_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    write_harbor(dir.path());
    let harbor = scan("harbor", dir.path());

    let (desired, notices) = desired_set(HarnessKind::ClaudeCode, std::slice::from_ref(&harbor));
    assert!(
        notices.is_empty(),
        "a native set raises no notices: {notices:?}"
    );
    // Nested command key preserved, agent stays `.md`, all passthrough.
    for (key, content) in [
        ("commands/charts/plot-route.md", HARBOR_COMMAND),
        ("agents/quartermaster.md", HARBOR_AGENT_MD),
    ] {
        let file = &desired.files[key];
        assert!(
            matches!(file.payload, DesiredPayload::File(_)),
            "{key} is passthrough"
        );
        assert_eq!(
            file.sha256,
            sha256_hex(content.as_bytes()),
            "{key} keeps the source hash"
        );
    }
    assert!(desired.mcps.contains_key("lighthouse"));
}

#[test]
fn claude_dialect_maps_to_codex_flattening_commands_and_rendering_agents() {
    let dir = tempfile::tempdir().unwrap();
    write_harbor(dir.path());
    let harbor = scan("harbor", dir.path());

    let (desired, notices) = desired_set(HarnessKind::Codex, std::slice::from_ref(&harbor));

    // The nested command flattens into Codex's flat prompt directory, keeping
    // its bytes exactly (one md dialect, only the key changes).
    let command = &desired.files["commands/charts-plot-route.md"];
    assert!(matches!(command.payload, DesiredPayload::File(_)));
    assert_eq!(command.sha256, sha256_hex(HARBOR_COMMAND.as_bytes()));

    // The markdown agent renders into Codex TOML, keyed `.toml`, hashed over the
    // rendered bytes - and model and tools drop with a notice each.
    let agent = &desired.files["agents/quartermaster.toml"];
    let DesiredPayload::Rendered(bytes) = &agent.payload else {
        panic!(
            "the agent must be a cross-dialect render, got {:?}",
            agent.payload
        );
    };
    let expected_toml = "name = \"quartermaster\"\n\
description = \"Keeps the ship's stores\"\n\
developer_instructions = '''\n\
Keep the stores in order.\n\
'''\n";
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected_toml);
    assert_eq!(agent.sha256, sha256_hex(expected_toml.as_bytes()));
    assert!(
        !desired.files.contains_key("agents/quartermaster.md"),
        "no md agent for Codex"
    );

    assert!(
        desired.mcps.contains_key("lighthouse"),
        "MCP registers for Codex"
    );
    assert!(
        notices
            .iter()
            .any(|n| n.contains("model") && n.contains("quartermaster")),
        "dropped `model` notice: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|n| n.contains("tools") && n.contains("quartermaster")),
        "dropped `tools` notice: {notices:?}"
    );
}

#[test]
fn claude_dialect_maps_to_copilot_passing_agents_and_skipping_commands() {
    let dir = tempfile::tempdir().unwrap();
    write_harbor(dir.path());
    let harbor = scan("harbor", dir.path());

    let (desired, notices) = desired_set(HarnessKind::Copilot, std::slice::from_ref(&harbor));

    // The markdown agent serves Copilot natively: passthrough, source hash.
    let agent = &desired.files["agents/quartermaster.md"];
    assert!(matches!(agent.payload, DesiredPayload::File(_)));
    assert_eq!(agent.sha256, sha256_hex(HARBOR_AGENT_MD.as_bytes()));
    // Copilot has no command surface.
    assert!(!desired.files.keys().any(|k| k.starts_with("commands/")));
    assert!(desired.mcps.contains_key("lighthouse"));
    assert!(
        notices
            .iter()
            .any(|n| n.contains("commands") && n.contains("GitHub Copilot CLI")),
        "skipped-command notice: {notices:?}"
    );
}

// --- Codex-authored set serves every harness ---------------------------------

#[test]
fn codex_dialect_passes_through_to_codex_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    write_cove(dir.path());
    let cove = scan("cove", dir.path());

    let (desired, notices) = desired_set(HarnessKind::Codex, std::slice::from_ref(&cove));
    assert!(
        notices.is_empty(),
        "a native set raises no notices: {notices:?}"
    );
    // The flat prompt stays flat; the TOML agent stays TOML. Both passthrough.
    let command = &desired.files["commands/deploy.md"];
    assert!(matches!(command.payload, DesiredPayload::File(_)));
    assert_eq!(command.sha256, sha256_hex(COVE_PROMPT.as_bytes()));
    let agent = &desired.files["agents/reviewer.toml"];
    assert!(matches!(agent.payload, DesiredPayload::File(_)));
    assert_eq!(agent.sha256, sha256_hex(COVE_AGENT_TOML.as_bytes()));
}

#[test]
fn codex_dialect_maps_to_claude_rendering_the_toml_agent_to_markdown() {
    let dir = tempfile::tempdir().unwrap();
    write_cove(dir.path());
    let cove = scan("cove", dir.path());

    let (desired, notices) = desired_set(HarnessKind::ClaudeCode, std::slice::from_ref(&cove));

    // The flat prompt installs as a flat Claude command, bytes unchanged.
    let command = &desired.files["commands/deploy.md"];
    assert!(matches!(command.payload, DesiredPayload::File(_)));
    assert_eq!(command.sha256, sha256_hex(COVE_PROMPT.as_bytes()));

    // The TOML agent renders into a markdown agent, keyed `.md`; model drops.
    let agent = &desired.files["agents/reviewer.md"];
    let DesiredPayload::Rendered(bytes) = &agent.payload else {
        panic!(
            "the agent must be a cross-dialect render, got {:?}",
            agent.payload
        );
    };
    let expected_md = "---\n\
name: reviewer\n\
description: Reviews code carefully\n\
---\n\n\
Be thorough.\n\
Name every risk.\n";
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), expected_md);
    assert_eq!(agent.sha256, sha256_hex(expected_md.as_bytes()));
    assert!(
        !desired.files.contains_key("agents/reviewer.toml"),
        "no toml agent for Claude"
    );
    assert!(
        notices
            .iter()
            .any(|n| n.contains("model") && n.contains("reviewer")),
        "dropped `model` notice: {notices:?}"
    );
}

#[test]
fn codex_dialect_maps_to_copilot_rendering_the_toml_agent_to_markdown() {
    let dir = tempfile::tempdir().unwrap();
    write_cove(dir.path());
    let cove = scan("cove", dir.path());

    let (desired, notices) = desired_set(HarnessKind::Copilot, std::slice::from_ref(&cove));

    // Copilot reads markdown agents, so the TOML agent renders down for it too.
    let agent = &desired.files["agents/reviewer.md"];
    assert!(matches!(agent.payload, DesiredPayload::Rendered(_)));
    // The prompt has no Copilot command surface.
    assert!(!desired.files.keys().any(|k| k.starts_with("commands/")));
    assert!(
        notices
            .iter()
            .any(|n| n.contains("commands") && n.contains("GitHub Copilot CLI")),
        "skipped-command notice: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|n| n.contains("model") && n.contains("reviewer")),
        "dropped `model` notice: {notices:?}"
    );
}

// --- flatten collision -------------------------------------------------------

#[test]
fn a_nested_and_flat_command_that_flatten_alike_collide_first_wins() {
    let dir = tempfile::tempdir().unwrap();
    // Both flatten to `charts-plot-route.md` for Codex; scan order decides the
    // winner, and the already-flat source sorts first (`-` before `/`).
    write(dir.path(), "commands/charts/plot-route.md", "nested\n");
    write(dir.path(), "commands/charts-plot-route.md", "flat\n");
    let domain = scan("harbor", dir.path());

    let (desired, notices) = desired_set(HarnessKind::Codex, std::slice::from_ref(&domain));

    // Exactly one command survives the collision under the flattened key.
    assert_eq!(
        desired
            .files
            .keys()
            .filter(|k| k.starts_with("commands/"))
            .count(),
        1
    );
    let winner = &desired.files["commands/charts-plot-route.md"];
    assert_eq!(
        winner.sha256,
        sha256_hex(b"flat\n"),
        "the flat source sorted first and won"
    );
    // The notice names both colliding source paths.
    assert!(
        notices
            .iter()
            .any(|n| n.contains("charts/plot-route.md") && n.contains("charts-plot-route.md")),
        "flatten-collision notice names both sources: {notices:?}"
    );
}

// --- cross-domain precedence still holds through translation -----------------

#[test]
fn cross_domain_collision_after_flattening_keeps_the_first_domain() {
    let harbor_dir = tempfile::tempdir().unwrap();
    let cove_dir = tempfile::tempdir().unwrap();
    // harbor ships a nested command that flattens to `deploy.md`... no; use the
    // same flattened key from two domains: harbor's nested `ops/deploy.md` and
    // cove's flat `ops-deploy.md` both become `ops-deploy.md` for Codex.
    write(harbor_dir.path(), "commands/ops/deploy.md", "harbor\n");
    write(cove_dir.path(), "commands/ops-deploy.md", "cove\n");
    let harbor = scan("harbor", harbor_dir.path());
    let cove = scan("cove", cove_dir.path());

    let (desired, notices) = desired_set(HarnessKind::Codex, &[harbor, cove]);
    let winner = &desired.files["commands/ops-deploy.md"];
    assert_eq!(
        winner.domain, "harbor",
        "the first domain in config order wins"
    );
    assert_eq!(winner.sha256, sha256_hex(b"harbor\n"));
    assert!(
        notices
            .iter()
            .any(|n| n.contains("harbor") && n.contains("cove")),
        "cross-domain notice names both domains: {notices:?}"
    );
}
