//! The Claude Desktop extension, from one source for both bundles.
//!
//! Crystalline reaches Claude Desktop as an MCP Bundle (`.mcpb`): a zip of a
//! `manifest.json`, an icon and the server it starts. Two bundles exist, and
//! both are described here so their manifests cannot drift apart:
//!
//! - [`Bundle::Hollow`] is built on the user's machine by
//!   `crystalline install claude-desktop`. It carries only a small Node
//!   launcher (`server/shim.js`) that starts the Crystalline already
//!   installed on this computer, and it can pin that binary's path through
//!   [`BINARY_ENV`].
//! - [`Bundle::DesktopOnly`] is the released macOS bundle for people who use
//!   only Claude Desktop. It carries the `crystalline` binary inside; the
//!   caller stages `server/crystalline` itself.
//!
//! Claude Desktop builds an extension's id from its author name and its
//! `name`, so [`NAME`], [`AUTHOR_NAME`] and [`AUTHOR_EMAIL`] never change: a
//! change would install a second extension beside the old one instead of
//! over it.
//!
//! [`pack`] writes the zip. Every entry gets the same date and mode, so the
//! same files always give the same bytes, and [`skill_zip`] uses it for the
//! Claude Desktop skill upload.

use serde_json::{Value, json};
use std::io::Write;

use crate::skills::SkillAsset;

/// The extension's `name`. Part of the id Claude Desktop derives, so it
/// never changes.
pub const NAME: &str = "crystalline";

/// The extension author's name, with the umlaut. Part of the id Claude
/// Desktop derives, so it never changes; a test pins it to the workspace's
/// first author.
pub const AUTHOR_NAME: &str = "Jordi Böhme";

/// The extension author's email.
pub const AUTHOR_EMAIL: &str = "jordi@boehme-lopez.de";

/// The environment variable the hollow extension sets to the path of the
/// Crystalline binary it was built for. The launcher tries that path first.
pub const BINARY_ENV: &str = "CRYSTALLINE_BINARY";

/// The Node launcher the hollow extension runs as `server/shim.js`.
pub const SHIM_JS: &str = include_str!("../../../packaging/mcpb/shim/shim.js");

/// The extension icon, `icon.png` in both bundles.
pub const ICON_PNG: &[u8] = include_bytes!("../../../assets/crystalline.png");

/// The tools both manifests declare, as `(name, description)` pairs in the
/// order Claude Desktop shows them. The descriptions are short hand-written
/// summaries, not the server's own tool descriptions. These are the tools a
/// default install lists: the six collaboration tools that `github.enabled`
/// gates are left out, and a service test checks the names against the
/// binary's tool list.
pub const TOOLS: &[(&str, &str)] = &[
    (
        "write_engram",
        "Capture a new engram, a unit of knowledge or experience, into a domain by writing its markdown file and indexing it.",
    ),
    (
        "edit_engram",
        "Refine an existing engram in place as understanding evolves, by section or with find and replace.",
    ),
    (
        "split_engram",
        "Move part of an engram into a new engram of its own when one engram mixes facts that change at different times.",
    ),
    (
        "move_engram",
        "Re-home an engram to a new path or domain, rewriting inbound links so nothing dangles.",
    ),
    (
        "delete_engram",
        "Remove an engram when its knowledge is retired, deleting the file and its index rows.",
    ),
    (
        "read_engram",
        "Read an engram's full markdown and resolved frontmatter to learn what is already known.",
    ),
    (
        "search_engrams",
        "Search across domains with hybrid lexical and semantic ranking to recall relevant knowledge.",
    ),
    (
        "build_context",
        "Assemble the neighbourhood around an anchor engram by following its relations and links.",
    ),
    (
        "recent_activity",
        "Review what has been captured recently across domains to catch up on new knowledge.",
    ),
    (
        "list_domains",
        "List the registered domains with their engram counts to see what the agent has been taught.",
    ),
    (
        "browse_domain",
        "Browse a domain's engrams by folder to explore how its knowledge is organized.",
    ),
    (
        "validate_engrams",
        "Check a domain's engrams against its schema engrams to keep captured knowledge well-formed; pass drift to also report observation categories and relation types in use but undeclared, and declared but unused.",
    ),
    (
        "infer_schema",
        "Suggest a Picoschema for a type by generalizing over engrams already captured in a domain.",
    ),
    (
        "vocabulary",
        "List the tags, observation categories and relation types already in use, with counts, so existing terms are reused.",
    ),
    (
        "evolve_engrams",
        "Sweep a domain for the maintenance the knowledge needs and return a ranked read-only work queue: lifecycle debt, structural gaps and near-duplicates, each with its evidence and the exact next action.",
    ),
    (
        "reflect",
        "Get the work queue of a domain's Reflection, its documents for people: what people changed, what changed in the engrams and threads waiting for an answer.",
    ),
    (
        "read_reflection",
        "Read a domain's Reflection: the tree of its documents for people, one document with its sections, source engrams and open threads, or the sections that match a search query. A page from a domain served by another machine carries its images as Markdown links only, no resource links.",
    ),
    (
        "write_reflection",
        "Create, change, move or delete a document of a domain's Reflection, or change one section of it, and record the engrams each section was built from.",
    ),
    (
        "discuss",
        "Open, answer, resolve or list threads on a passage of a Reflection document, written as conventional comments for the people who read it.",
    ),
    (
        "configure",
        "View and adjust Crystalline's settings, like connecting a GitHub account for team collaboration.",
    ),
    (
        "add_domain",
        "Create or connect a domain to capture engrams in: a local folder of markdown files, a database-backed virtual domain or a GitHub team domain.",
    ),
    (
        "remove_domain",
        "Unregister a domain the agent should stop learning from; a local folder's markdown files stay where they are.",
    ),
    (
        "provision",
        "Install the skills, commands, agents and MCP servers a domain ships into the user's coding harnesses, after the user allows it.",
    ),
    (
        "skills",
        "List the agent skills this server ships and read any skill's full playbook before its kind of task.",
    ),
];

/// One short line about Crystalline, the manifest's `description`.
const DESCRIPTION: &str = "Crystalline intelligence for AI agents: teach knowledge in Domains, capture learnings as Engrams.";

/// The first sentences of the hollow extension's `long_description`.
const HOLLOW_LEAD: &str = "This extension connects Claude Desktop to the Crystalline installed on this computer. It was built by `crystalline install claude-desktop`, which also updates it.";

/// The first sentences of the Desktop-only bundle's `long_description`.
const DESKTOP_ONLY_LEAD: &str = "This macOS extension carries Crystalline inside, for people who use only Claude Desktop. If you also use Claude Code or the crystalline command, install Crystalline with Homebrew and run `crystalline install claude-desktop` instead.";

/// The text both bundles share after their lead.
const LONG_REST: &str = "In psychology, fluid intelligence reasons in the moment while crystallized intelligence is what learning has accumulated. A language model is all fluid; Crystalline gives an AI agent the crystalline half: knowledge that endures across sessions instead of starting from zero each time. The moment it connects, the server's own instructions carry a live routing index, so onboarding is automatic; from there the agent is taught information through curated Domains and captures what it learns and experiences as Engrams: markdown files with structured frontmatter that stay readable and editable outside of any agent.\n\nOver time an engram collection becomes a crystalline intelligence the agent can search, browse and build context from before starting new work, turning it into a more useful peer with each session it runs. It starts with no domains: the agent creates one whenever it needs somewhere to capture knowledge, with the add_domain tool, as a folder of markdown files under your Documents/Crystalline folder, a database-backed domain or a GitHub team domain. A companion skill teaches Claude capture and collaboration best practices: `crystalline install claude-desktop` saves it for upload, and each release carries it as crystalline-claude-desktop-skill. See the README's Skills section.";

/// Which of the two extensions to describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bundle<'a> {
    /// The extension `crystalline install claude-desktop` builds: the Node
    /// launcher only. `binary` is the Crystalline it pins through
    /// [`BINARY_ENV`]; `None` leaves the launcher to its own search.
    Hollow {
        /// The path of the Crystalline binary the launcher tries first.
        binary: Option<&'a str>,
    },
    /// The released macOS bundle with the `crystalline` binary inside.
    DesktopOnly,
}

/// The `manifest.json` of `bundle` at `version` (a release version without a
/// leading `v`).
pub fn manifest(bundle: Bundle<'_>, version: &str) -> Value {
    let (display_name, lead, server, platforms) = match bundle {
        Bundle::Hollow { binary } => {
            let mut config = json!({
                "command": "node",
                "args": ["${__dirname}/server/shim.js"],
            });
            if let Some(binary) = binary {
                config["env"] = json!({ BINARY_ENV: binary });
            }
            (
                "Crystalline",
                HOLLOW_LEAD,
                json!({
                    "type": "node",
                    "entry_point": "server/shim.js",
                    "mcp_config": config,
                }),
                json!(["darwin", "win32"]),
            )
        }
        Bundle::DesktopOnly => (
            "Crystalline (Claude Desktop only)",
            DESKTOP_ONLY_LEAD,
            json!({
                "type": "binary",
                "entry_point": "server/crystalline",
                "mcp_config": {
                    "command": "${__dirname}/server/crystalline",
                    "args": ["mcp"],
                    "env": { "CRYSTALLINE_CHANNEL": "mcpb" },
                },
            }),
            json!(["darwin"]),
        ),
    };
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|(name, description)| json!({ "name": name, "description": description }))
        .collect();
    json!({
        "manifest_version": "0.3",
        "name": NAME,
        "display_name": display_name,
        "version": version,
        "description": DESCRIPTION,
        "long_description": format!("{lead} {LONG_REST}"),
        "author": {
            "name": AUTHOR_NAME,
            "email": AUTHOR_EMAIL,
        },
        "repository": {
            "type": "git",
            "url": "https://github.com/jordiboehme/crystalline.git",
        },
        "homepage": "https://github.com/jordiboehme/crystalline",
        "documentation": "https://github.com/jordiboehme/crystalline/tree/main/docs",
        "support": "https://github.com/jordiboehme/crystalline/issues",
        "icon": "icon.png",
        "license": "MPL-2.0",
        "privacy_policies": [
            "https://github.com/jordiboehme/crystalline#privacy-policy",
            "https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement",
            "https://huggingface.co/privacy",
        ],
        "keywords": ["knowledge", "memory", "agent", "mcp", "markdown"],
        "server": server,
        "compatibility": {
            "claude_desktop": ">=0.10.0",
            "platforms": platforms,
        },
        "tools": tools,
        "tools_generated": false,
    })
}

/// The files of `bundle` at `version`, as `(path inside the bundle, bytes)`:
/// `manifest.json` (pretty JSON with a trailing newline), `icon.png` and,
/// for [`Bundle::Hollow`], `server/shim.js`. Never `server/crystalline`: for
/// [`Bundle::DesktopOnly`] the caller stages the binary.
pub fn files(bundle: Bundle<'_>, version: &str) -> Vec<(&'static str, Vec<u8>)> {
    let mut text = serde_json::to_string_pretty(&manifest(bundle, version))
        .expect("a JSON value always serializes");
    text.push('\n');
    let mut files = vec![
        ("manifest.json", text.into_bytes()),
        ("icon.png", ICON_PNG.to_vec()),
    ];
    if let Bundle::Hollow { .. } = bundle {
        files.push(("server/shim.js", SHIM_JS.as_bytes().to_vec()));
    }
    files
}

/// A zip of `files`, each `(path inside the zip, bytes)` in the given order.
/// Every entry is deflated and dated `zip::DateTime::default()` with mode
/// 0o644, so the same files always give the same bytes.
pub fn pack(files: &[(&str, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, content) in files {
        writer.start_file(*path, options)?;
        writer.write_all(content)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// The zip Claude Desktop takes as a skill upload: one folder named after the
/// skill at its root, holding the skill's `SKILL.md`.
pub fn skill_zip(skill: &SkillAsset) -> std::io::Result<Vec<u8>> {
    let path = format!("{}/SKILL.md", skill.name);
    pack(&[(path.as_str(), skill.content.as_bytes().to_vec())])
}

/// The lowercase hex sha256 of [`SHIM_JS`].
pub fn shim_sha256() -> String {
    sha256_hex(SHIM_JS.as_bytes())
}

/// The lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::provision::receipt::sha256_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills;
    use serde_json::{Value, json};
    use std::io::Read;

    const PINNED: &str = "/opt/homebrew/bin/crystalline";

    fn all_bundles() -> [Bundle<'static>; 3] {
        [
            Bundle::Hollow {
                binary: Some(PINNED),
            },
            Bundle::Hollow { binary: None },
            Bundle::DesktopOnly,
        ]
    }

    /// Claude Desktop builds the extension id from the author name and
    /// `name`, so both stay exactly what the old extension carried, and the
    /// author constants stay the workspace's first author.
    #[test]
    fn both_bundles_install_over_the_old_extension() {
        for bundle in all_bundles() {
            let m = manifest(bundle, "9.9.9");
            assert_eq!(m["name"], "crystalline", "{bundle:?}");
            assert_eq!(m["author"]["name"], "Jordi Böhme", "{bundle:?}");
            assert_eq!(m["author"]["email"], "jordi@boehme-lopez.de", "{bundle:?}");
        }
        let first = env!("CARGO_PKG_AUTHORS")
            .split(':')
            .next()
            .expect("the crate has an author");
        let (name, email) = first
            .split_once(" <")
            .expect("the first author reads Name <email>");
        assert_eq!(AUTHOR_NAME, name);
        assert_eq!(AUTHOR_EMAIL, email.trim_end_matches('>'));
        assert_eq!(NAME, "crystalline");
    }

    #[test]
    fn the_hollow_manifest_runs_the_shim_and_pins_the_binary() {
        let m = manifest(
            Bundle::Hollow {
                binary: Some(PINNED),
            },
            "9.9.9",
        );
        assert_eq!(m["display_name"], "Crystalline");
        assert_eq!(m["version"], "9.9.9");
        assert_eq!(m["server"]["type"], "node");
        assert_eq!(m["server"]["entry_point"], "server/shim.js");
        let config = &m["server"]["mcp_config"];
        assert_eq!(config["command"], "node");
        assert_eq!(config["args"], json!(["${__dirname}/server/shim.js"]));
        assert_eq!(config["env"], json!({ BINARY_ENV: PINNED }));
        assert_eq!(config["env"], json!({ "CRYSTALLINE_BINARY": PINNED }));
        assert_eq!(m["compatibility"]["platforms"], json!(["darwin", "win32"]));
        assert!(m["compatibility"].get("runtimes").is_none());
        assert!(m.get("platform_overrides").is_none());
        assert!(m["server"].get("platform_overrides").is_none());

        let unpinned = manifest(Bundle::Hollow { binary: None }, "9.9.9");
        let config = unpinned["server"]["mcp_config"].as_object().unwrap();
        assert!(!config.contains_key("env"), "{config:?}");
        assert_eq!(unpinned["server"]["type"], "node");
    }

    #[test]
    fn the_desktop_only_manifest_runs_its_own_binary() {
        let m = manifest(Bundle::DesktopOnly, "9.9.9");
        assert_eq!(m["display_name"], "Crystalline (Claude Desktop only)");
        assert_eq!(m["version"], "9.9.9");
        assert_eq!(m["server"]["type"], "binary");
        assert_eq!(m["server"]["entry_point"], "server/crystalline");
        let config = &m["server"]["mcp_config"];
        assert_eq!(config["command"], "${__dirname}/server/crystalline");
        assert_eq!(config["args"], json!(["mcp"]));
        assert_eq!(config["env"], json!({ "CRYSTALLINE_CHANNEL": "mcpb" }));
        assert_eq!(m["compatibility"]["platforms"], json!(["darwin"]));
    }

    #[test]
    fn the_tools_block_matches_the_constant() {
        assert_eq!(TOOLS.len(), 24);
        let expected: Vec<Value> = TOOLS
            .iter()
            .map(|(name, description)| json!({ "name": name, "description": description }))
            .collect();
        for bundle in all_bundles() {
            let m = manifest(bundle, "9.9.9");
            assert_eq!(m["tools"], Value::Array(expected.clone()), "{bundle:?}");
            assert_eq!(m["tools_generated"], false, "{bundle:?}");
        }
    }

    fn names(files: &[(&'static str, Vec<u8>)]) -> Vec<&'static str> {
        files.iter().map(|(name, _)| *name).collect()
    }

    #[test]
    fn the_hollow_files_are_the_manifest_icon_and_shim() {
        let bundle = Bundle::Hollow {
            binary: Some(PINNED),
        };
        let hollow = files(bundle, "9.9.9");
        assert_eq!(
            names(&hollow),
            ["manifest.json", "icon.png", "server/shim.js"]
        );
        let (_, shim) = hollow.iter().find(|(n, _)| *n == "server/shim.js").unwrap();
        assert_eq!(shim.as_slice(), SHIM_JS.as_bytes());
        let (_, icon) = hollow.iter().find(|(n, _)| *n == "icon.png").unwrap();
        assert_eq!(icon.as_slice(), ICON_PNG);
        let (_, text) = hollow.iter().find(|(n, _)| *n == "manifest.json").unwrap();
        let text = std::str::from_utf8(text).unwrap();
        assert!(text.ends_with("}\n"), "a trailing newline");
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, manifest(bundle, "9.9.9"));
        assert_eq!(
            text,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&manifest(bundle, "9.9.9")).unwrap()
            )
        );

        let desktop_only = files(Bundle::DesktopOnly, "9.9.9");
        assert_eq!(names(&desktop_only), ["manifest.json", "icon.png"]);
    }

    fn unpack(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|i| {
                let mut entry = archive.by_index(i).unwrap();
                let mut content = Vec::new();
                entry.read_to_end(&mut content).unwrap();
                (entry.name().unwrap().to_string(), content)
            })
            .collect()
    }

    #[test]
    fn packing_is_deterministic() {
        let staged = files(
            Bundle::Hollow {
                binary: Some(PINNED),
            },
            "9.9.9",
        );
        let first = pack(&staged).unwrap();
        let second = pack(&staged).unwrap();
        assert_eq!(first, second);
        let read: Vec<(String, Vec<u8>)> = unpack(&first);
        let given: Vec<(String, Vec<u8>)> = staged
            .iter()
            .map(|(name, content)| (name.to_string(), content.clone()))
            .collect();
        assert_eq!(read, given);

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(first.as_slice())).unwrap();
        for i in 0..archive.len() {
            let entry = archive.by_index(i).unwrap();
            assert_eq!(entry.compression(), zip::CompressionMethod::Deflated);
            assert_eq!(entry.unix_mode().map(|m| m & 0o777), Some(0o644));
        }
    }

    #[test]
    fn the_skill_zip_holds_one_folder_at_its_root() {
        let asset = skills::skill("crystalline-intelligence").unwrap();
        let bytes = skill_zip(asset).unwrap();
        assert_eq!(
            unpack(&bytes),
            [(
                "crystalline-intelligence/SKILL.md".to_string(),
                asset.content.as_bytes().to_vec()
            )]
        );
    }

    #[test]
    fn the_copy_carries_no_dash() {
        let dashes = ['\u{2014}', '\u{2013}'];
        for bundle in all_bundles() {
            let m = manifest(bundle, "9.9.9");
            for key in ["long_description", "description"] {
                let text = m[key].as_str().unwrap();
                assert!(!text.contains(dashes), "{bundle:?} {key}: {text}");
            }
        }
        for (name, description) in TOOLS {
            assert!(!description.contains(dashes), "{name}: {description}");
        }
    }

    #[test]
    fn the_leads_open_the_long_description() {
        let hollow = manifest(Bundle::Hollow { binary: None }, "9.9.9");
        let desktop_only = manifest(Bundle::DesktopOnly, "9.9.9");
        let hollow = hollow["long_description"].as_str().unwrap();
        let desktop_only = desktop_only["long_description"].as_str().unwrap();
        assert!(hollow.starts_with(
            "This extension connects Claude Desktop to the Crystalline installed on this computer."
        ));
        assert!(desktop_only.starts_with("This macOS extension carries Crystalline inside"));
        assert!(hollow.ends_with("See the README's Skills section."));
        assert!(hollow.contains("outside of any agent.\n\nOver time"));
    }

    #[test]
    fn the_hashes_are_lowercase_hex_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(shim_sha256(), sha256_hex(SHIM_JS.as_bytes()));
    }
}
