//! The agent skills this binary ships, embedded as static assets.
//!
//! A skill is a folder under `skills/` with a `SKILL.md` playbook teaching one
//! kind of Crystalline work: routing to a domain, capturing what was learned,
//! modelling a schema, collaborating with a team, provisioning what a domain
//! ships. `include_str!` bakes each one
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
//! the five topical skills - installing both would teach the same lessons twice.
//! It is still served over MCP like any other: a remote client reading the
//! skills should see everything this binary knows how to teach.
//!
//! [`SkillAsset::since`] names the release that first shipped a skill. The
//! session-start upgrade otherwise reads a managed skill missing on disk as a
//! deliberate deletion; `since` lets it tell "removed by the person" from
//! "never offered to this install" and add a skill new in this release.

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
    /// The first release that ships this skill. An upgrade from an older
    /// install adds the skill even though it is missing on disk; see
    /// `reconcile_skill_set` in the CLI. Always a release triple, never a
    /// pre-release.
    pub since: &'static str,
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
/// with a team, provision what a domain ships, and the consolidated Claude
/// Desktop skill last.
pub const SKILL_ASSETS: &[SkillAsset] = &[
    SkillAsset {
        name: "crystalline-routing",
        content: include_str!("../../../skills/crystalline-routing/SKILL.md"),
        install_managed: true,
        since: "0.1.0",
    },
    SkillAsset {
        name: "crystalline-capture",
        content: include_str!("../../../skills/crystalline-capture/SKILL.md"),
        install_managed: true,
        since: "0.1.0",
    },
    SkillAsset {
        name: "crystalline-schema",
        content: include_str!("../../../skills/crystalline-schema/SKILL.md"),
        install_managed: true,
        since: "0.1.0",
    },
    SkillAsset {
        name: "crystalline-collaboration",
        content: include_str!("../../../skills/crystalline-collaboration/SKILL.md"),
        install_managed: true,
        since: "0.1.0",
    },
    SkillAsset {
        name: "crystalline-provisioning",
        content: include_str!("../../../skills/crystalline-provisioning/SKILL.md"),
        install_managed: true,
        since: "0.22.1",
    },
    SkillAsset {
        name: "crystalline-intelligence",
        content: include_str!("../../../skills/crystalline-intelligence/SKILL.md"),
        install_managed: false,
        since: "0.1.0",
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
    fn every_skill_names_the_version_that_first_shipped_it() {
        for s in SKILL_ASSETS {
            assert!(!s.since.is_empty(), "{} has no since", s.name);
            assert!(
                s.since.split('.').count() == 3 && !s.since.contains('-'),
                "{}: since is a release triple, never a pre-release",
                s.name
            );
        }
    }

    /// `major.minor.patch` of a version string, any pre-release or build
    /// suffix dropped, so a dev-channel build compares as its release.
    fn release_triple(version: &str) -> (u64, u64, u64) {
        let core = version.split(['-', '+']).next().unwrap_or_default();
        let mut parts = core.split('.').map(|p| p.parse::<u64>().unwrap());
        let triple = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        assert!(parts.next().is_none(), "{version} is not a triple");
        triple
    }

    /// A skill whose `since` is newer than the binary that ships it is
    /// recorded under the older version at install, so the upgrade rule
    /// never runs for it and the real release later brings back a copy the
    /// person removed. The release bump has to come with the skill.
    #[test]
    fn no_skill_is_newer_than_the_binary_that_ships_it() {
        let binary = release_triple(env!("CARGO_PKG_VERSION"));
        for s in SKILL_ASSETS {
            assert!(
                release_triple(s.since) <= binary,
                "{} names since {} but this build is {}",
                s.name,
                s.since,
                env!("CARGO_PKG_VERSION")
            );
        }
    }

    #[test]
    fn release_triple_drops_a_pre_release_suffix() {
        assert_eq!(release_triple("0.22.1-dev.20261002"), (0, 22, 1));
        assert_eq!(release_triple("0.22.1"), (0, 22, 1));
        assert!(release_triple("0.22.1") > release_triple("0.22.0"));
        assert!(release_triple("0.10.0") > release_triple("0.9.9"));
    }

    #[test]
    fn six_skills_ship_with_five_installed_into_harnesses() {
        let names: Vec<&str> = SKILL_ASSETS.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec![
                "crystalline-routing",
                "crystalline-capture",
                "crystalline-schema",
                "crystalline-collaboration",
                "crystalline-provisioning",
                "crystalline-intelligence",
            ]
        );
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
                "crystalline-provisioning",
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

    #[test]
    fn the_skills_teach_the_cross_domain_link_form() {
        let capture = skill("crystalline-capture").unwrap().content;
        assert!(
            capture.contains("`[[<domain>:<Title or permalink>]]`"),
            "capture skill lacks the link form"
        );
        assert!(
            capture.contains("This rule covers tool arguments only"),
            "capture skill still reads like a ban"
        );
        let desktop = skill("crystalline-intelligence").unwrap().content;
        assert!(
            desktop.contains("`[[domain:Target]]`"),
            "consolidated skill lacks the link form"
        );
        assert!(
            desktop.contains("`[[<domain>:<Title or permalink>]]`"),
            "consolidated skill lacks the relation form"
        );
    }

    #[test]
    fn the_skills_teach_that_a_miss_names_the_permalink() {
        let hint = "A file path is not an identifier: a miss names the permalink it probably meant";
        for name in ["crystalline-capture", "crystalline-intelligence"] {
            let content = skill(name).unwrap().content;
            assert!(content.contains(hint), "{name} lacks the miss hint");
            assert!(
                !content.contains("file path inside the domain"),
                "{name} still offers the file path as an identifier"
            );
            assert!(
                !content.contains("with or without `.md`"),
                "{name} still offers the file path as an identifier"
            );
        }
        let routing = skill("crystalline-routing").unwrap().content;
        assert!(
            routing.contains(
                "The filename `MANIFEST.md` is not an identifier, and a miss names the permalink it probably meant"
            ),
            "routing skill calls MANIFEST.md no identifier and names the hint"
        );
        assert!(
            !routing.contains("`MANIFEST.md` resolves"),
            "routing skill no longer says MANIFEST.md resolves"
        );
    }

    #[test]
    fn the_capture_skill_lists_the_provenance_keys_as_settable() {
        let capture = skill("crystalline-capture").unwrap().content;
        assert!(
            capture
                .contains("`source_date`, `resource`, `source_version`, `salience` and `verified`"),
            "capture skill lists resource and source_version as settable"
        );
    }

    #[test]
    fn the_skills_teach_per_engram_provenance_and_citing_it() {
        let capture = skill("crystalline-capture").unwrap().content;
        for needle in [
            "- ingested_from [[",
            "`[source]`",
            "/blob/{sha}/{path}",
            "V111",
        ] {
            assert!(capture.contains(needle), "capture skill lacks {needle}");
        }
        let routing = skill("crystalline-routing").unwrap().content;
        assert!(
            routing.contains("cite the location"),
            "routing skill lacks the citing rule"
        );
        let desktop = skill("crystalline-intelligence").unwrap().content;
        for needle in ["ingested_from", "cite that location"] {
            assert!(
                desktop.contains(needle),
                "consolidated skill lacks {needle}"
            );
        }
    }

    #[test]
    fn the_provisioning_skill_keeps_its_guardrails() {
        let s = skill("crystalline-provisioning").expect("ships").content;
        for (needle, why) in [
            (
                "\"identifier\": \"manifest\"",
                "the MANIFEST is edited over MCP by its permalink",
            ),
            (
                "\"operation\": \"append\"",
                "a new Provisioning section is appended",
            ),
            (
                "replace_section",
                "an existing section is replaced, body only",
            ),
            (
                "expected_checksum",
                "the edit is guarded against a concurrent change",
            ),
            (
                "manifest_findings",
                "the agent fixes every finding before going on",
            ),
            ("Copy, never move", "a moved skill disappears on deny"),
            (
                "only after the person answers",
                "allow or deny is the person's decision",
            ),
            ("secret", "no secret value in an mcps file"),
            (
                "repository root",
                "a team ../ path never climbs above the repo root",
            ),
            (
                "crystalline-",
                "the reserved prefix is named so the agent avoids it",
            ),
        ] {
            assert!(s.contains(needle), "{why}: missing {needle:?}");
        }
        assert_eq!(skill("crystalline-provisioning").unwrap().since, "0.22.1");
    }

    /// Each skill names the call that shows and changes how a domain
    /// behaves.
    #[test]
    fn the_skills_teach_configure_with_a_domain() {
        let sentence = "To see or change how a domain behaves (its sharing, generated indexes, MANIFEST sections or rule overrides), call configure with the domain.";
        for name in [
            "crystalline-intelligence",
            "crystalline-provisioning",
            "crystalline-collaboration",
        ] {
            let body = skill(name).unwrap().content;
            assert!(body.contains(sentence), "{name}");
        }
    }
}
