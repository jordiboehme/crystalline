//! The agent skills this binary ships, embedded as static assets.
//!
//! A skill is a folder under `skills/` with a `SKILL.md` playbook teaching one
//! kind of Crystalline work: routing to a domain, capturing what was learned,
//! modelling a schema, collaborating with a team. `include_str!` bakes each one
//! into the binary, so an install from a downloaded release carries exactly the
//! skills a clone would.
//!
//! The assets live in core because two very different consumers need the same
//! bytes. `crystalline install` copies the managed ones into a harness's skills
//! folder, where the harness loads them itself. The MCP server serves all of
//! them to a remote client that never runs the CLI at all: as `skill://`
//! resources, as the `skills` tool's index and full-text reads, and as the
//! shape a future ratified skills extension would advertise. Keeping the list
//! here means neither consumer can drift from the other, and core stays static
//! (no async, no database, no ML) the way the rest of the crate is.
//!
//! [`SkillAsset::install_managed`] is the one axis the two consumers differ on.
//! `crystalline-intelligence` is the single consolidated skill for Claude
//! Desktop, which has no hooks and installs one skill at a time, so it ships
//! only as its own zip and is never copied into a harness skills folder beside
//! the four topical skills - installing both would teach the same lessons twice.
//! It is still served over MCP like any other: a remote client reading the
//! skills should see everything this binary knows how to teach.

/// One shipped agent skill: its folder name, its embedded `SKILL.md` and
/// whether `crystalline install` copies it into a harness skills folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillAsset {
    /// The skill folder name, which is also its frontmatter `name` and the
    /// name it is served under (`skill://<name>/SKILL.md`).
    pub name: &'static str,
    /// The full `SKILL.md`, frontmatter included, exactly as it ships.
    pub content: &'static str,
    /// Whether `crystalline install` copies this skill into a harness's
    /// skills folder. False for the consolidated Claude Desktop skill, which
    /// ships as its own zip; see the module docs.
    pub install_managed: bool,
}

impl SkillAsset {
    /// The skill's one-line `description` from its frontmatter: what a harness
    /// (or an agent reading the `skills` index) uses to decide whether this
    /// playbook applies. Every shipped skill has exactly one such line; an
    /// asset that somehow lost it reads as an empty description rather than
    /// failing, since a missing line is a copy problem, never a runtime one.
    pub fn description(&self) -> &str {
        self.content
            .lines()
            .find_map(|line| line.strip_prefix("description:"))
            .map(str::trim)
            .unwrap_or_default()
    }
}

/// Every skill this binary ships, in the order a reader should meet them:
/// route to a domain, capture what was learned, model a schema, collaborate
/// with a team, and the consolidated Claude Desktop skill last.
pub const SKILL_ASSETS: &[SkillAsset] = &[
    SkillAsset {
        name: "crystalline-routing",
        content: include_str!("../../../skills/crystalline-routing/SKILL.md"),
        install_managed: true,
    },
    SkillAsset {
        name: "crystalline-capture",
        content: include_str!("../../../skills/crystalline-capture/SKILL.md"),
        install_managed: true,
    },
    SkillAsset {
        name: "crystalline-schema",
        content: include_str!("../../../skills/crystalline-schema/SKILL.md"),
        install_managed: true,
    },
    SkillAsset {
        name: "crystalline-collaboration",
        content: include_str!("../../../skills/crystalline-collaboration/SKILL.md"),
        install_managed: true,
    },
    SkillAsset {
        name: "crystalline-intelligence",
        content: include_str!("../../../skills/crystalline-intelligence/SKILL.md"),
        install_managed: false,
    },
];

/// Look one shipped skill up by name, or `None` when nothing ships under that
/// name.
pub fn skill(name: &str) -> Option<&'static SkillAsset> {
    SKILL_ASSETS.iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_skills_ship_with_four_installed_into_harnesses() {
        assert_eq!(SKILL_ASSETS.len(), 5);
        let managed: Vec<&str> = SKILL_ASSETS
            .iter()
            .filter(|s| s.install_managed)
            .map(|s| s.name)
            .collect();
        assert_eq!(
            managed,
            vec![
                "crystalline-routing",
                "crystalline-capture",
                "crystalline-schema",
                "crystalline-collaboration",
            ]
        );
        assert_eq!(
            skill("crystalline-intelligence").map(|s| s.install_managed),
            Some(false),
            "the consolidated Desktop skill is served but never installed"
        );
    }

    #[test]
    fn every_asset_carries_its_own_frontmatter_name_and_a_description() {
        for asset in SKILL_ASSETS {
            let name_line = asset
                .content
                .lines()
                .find_map(|l| l.strip_prefix("name:"))
                .map(str::trim)
                .unwrap_or_default();
            assert_eq!(
                name_line, asset.name,
                "{}: frontmatter name must match the folder name",
                asset.name
            );
            assert!(
                !asset.description().is_empty(),
                "{}: a skill without a description cannot be routed to",
                asset.name
            );
        }
    }

    #[test]
    fn skill_looks_up_by_name_and_misses_cleanly() {
        let routing = skill("crystalline-routing").expect("the routing skill ships");
        assert!(routing.content.starts_with("---\n"));
        assert!(skill("crystalline-nonesuch").is_none());
    }
}
