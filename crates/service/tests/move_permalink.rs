//! A move is a refactoring: the permalink can follow the engram, and every
//! reference follows the address.
//!
//! Driven through the engine over two file domains (`notes`, `ops`) and one
//! virtual domain (`vault`), so both storage kinds are covered at both ends of
//! a move. The last test is issue 92's own scenario end to end: four engrams
//! whose permalinks drifted off their folder, repaired in place, found by a
//! folder glob afterwards and still resolving from everything that pointed at
//! them.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::*;
use serde_json::Value;
use tokio::sync::Mutex;

/// An engram with an explicit permalink, the way the write tools lay one out.
fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\nstatus: stable\n\
         recorded_at: 2026-01-01\ngenerated: {{by: human:jordi, at: 2026-01-01T09:00:00Z}}\n\
         ---\n\n# {title}\n\n{body}\n"
    )
}

fn seed(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

struct Fixture {
    engine: Engine,
    notes: tempfile::TempDir,
    ops: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Fixture {
        let notes = tempfile::tempdir().unwrap();
        let ops = tempfile::tempdir().unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.domains.insert(
            "notes".to_string(),
            DomainEntry::file(notes.path().to_path_buf()),
        );
        cfg.domains.insert(
            "ops".to_string(),
            DomainEntry::file(ops.path().to_path_buf()),
        );
        cfg.domains
            .insert("vault".to_string(), DomainEntry::virtual_domain());
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, None);
        Fixture { engine, notes, ops }
    }

    fn notes(&self) -> &Path {
        self.notes.path()
    }

    fn ops(&self) -> &Path {
        self.ops.path()
    }

    async fn sync(&self) {
        self.engine.sync(None).await.unwrap();
    }

    async fn mv(
        &self,
        identifier: &str,
        domain: &str,
        destination: &str,
        destination_domain: Option<&str>,
        permalink: Option<&str>,
    ) -> Result<Value, String> {
        self.engine
            .move_engram_as(
                &MoveParams {
                    identifier: identifier.to_string(),
                    domain: domain.to_string(),
                    destination: destination.to_string(),
                    destination_domain: destination_domain.map(str::to_string),
                    permalink: permalink.map(str::to_string),
                    update_links: None,
                },
                Some("mover"),
                &Scope::Unrestricted,
            )
            .await
            .map_err(|e| e.to_string())
    }

    /// The full markdown an engram serves, by address.
    async fn content(&self, domain: &str, identifier: &str) -> String {
        let read = self
            .engine
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some(domain.to_string()),
                    share_link: None,
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap_or_else(|e| panic!("{domain}:{identifier} should read: {e}"));
        read["content"].as_str().unwrap().to_string()
    }

    async fn missing(&self, domain: &str, identifier: &str) -> bool {
        self.engine
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some(domain.to_string()),
                    share_link: None,
                },
                &Scope::Unrestricted,
            )
            .await
            .is_err()
    }

    /// The permalinks a `build_context` glob anchors on, sorted; empty when
    /// the glob matches nothing.
    async fn glob(&self, anchor: &str) -> Vec<String> {
        let out = self
            .engine
            .build_context(
                &ContextParams {
                    anchor: anchor.to_string(),
                    depth: Some(1),
                    domains: Vec::new(),
                    timeframe: None,
                    max_related: Some(0),
                },
                &Scope::Unrestricted,
            )
            .await;
        let prefix = anchor
            .trim_start_matches("crystalline://notes/")
            .trim_end_matches('*');
        let mut found: Vec<String> = match out {
            Ok(out) => out["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["permalink"].as_str().unwrap().to_string())
                .filter(|p| p.starts_with(prefix))
                .collect(),
            Err(_) => Vec::new(),
        };
        found.sort();
        found
    }

    /// The unresolved references the maintenance sweep reports in `domain`.
    async fn unresolved(&self, domain: &str) -> Vec<String> {
        self.findings(domain, "V102").await
    }

    /// One rule's findings in `domain`, as permalink and evidence.
    async fn findings(&self, domain: &str, rule: &str) -> Vec<String> {
        let v = self
            .engine
            .evolve_detect(
                &EvolveParams {
                    domains: vec![domain.to_string()],
                    rules: vec![rule.to_string()],
                    limit: Some(50),
                    ..EvolveParams::default()
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap();
        v["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| format!("{} {}", row["permalink"], row["evidence"]))
            .collect()
    }
}

// --- the permalink the move lands with ----------------------------------------

#[tokio::test]
async fn a_permalink_in_step_with_its_path_follows_the_move() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "alpha.md",
        &engram("Alpha Notes", "alpha", "the alpha body"),
    );
    f.sync().await;

    let receipt = f
        .mv("alpha", "notes", "guides/alpha.md", None, None)
        .await
        .unwrap();
    assert_eq!(receipt["to"]["permalink"], "guides/alpha", "{receipt}");
    let text = read(f.notes(), "guides/alpha.md");
    assert!(text.contains("permalink: guides/alpha\n"), "{text}");
    assert!(f.missing("notes", "alpha").await);
}

#[tokio::test]
async fn a_custom_permalink_stays_by_default() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "alpha.md",
        &engram("Alpha", "handbook/alpha", "the alpha body"),
    );
    f.sync().await;

    let receipt = f
        .mv("handbook/alpha", "notes", "guides/alpha.md", None, None)
        .await
        .unwrap();
    assert_eq!(receipt["to"]["permalink"], "handbook/alpha", "{receipt}");
    assert_eq!(receipt["to"]["path"], "guides/alpha.md");
    let text = read(f.notes(), "guides/alpha.md");
    assert!(text.contains("permalink: handbook/alpha\n"), "{text}");
    assert!(
        !text.contains("by: mover"),
        "a move that leaves the text alone leaves its provenance alone: {text}"
    );
}

#[tokio::test]
async fn path_keep_and_a_named_permalink_each_do_what_they_say() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("A", "custom/a", "the a body"));
    seed(f.notes(), "b.md", &engram("B", "b", "the b body"));
    seed(f.notes(), "c.md", &engram("C", "c", "the c body"));
    f.sync().await;

    let path = f
        .mv("custom/a", "notes", "topics/a.md", None, Some("path"))
        .await
        .unwrap();
    assert_eq!(path["to"]["permalink"], "topics/a");

    // Kept, although it was in step and would otherwise follow.
    let keep = f
        .mv("b", "notes", "topics/b.md", None, Some("keep"))
        .await
        .unwrap();
    assert_eq!(keep["to"]["permalink"], "b");
    assert!(read(f.notes(), "topics/b.md").contains("permalink: b\n"));

    let named = f
        .mv("c", "notes", "topics/c.md", None, Some("reference/c-notes"))
        .await
        .unwrap();
    assert_eq!(named["to"]["permalink"], "reference/c-notes");
    assert!(read(f.notes(), "topics/c.md").contains("permalink: reference/c-notes\n"));

    // Every one of them resolves at its new address after a full resync too,
    // which is the proof the file and the row say the same thing.
    f.sync().await;
    for address in ["topics/a", "b", "reference/c-notes"] {
        assert!(
            !f.missing("notes", address).await,
            "{address} resolves after a resync"
        );
    }
}

#[tokio::test]
async fn a_permalink_that_is_not_one_is_refused_before_anything_moves() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("A", "a", "the a body"));
    f.sync().await;
    for bad in ["notes:a", "crystalline://notes/a", "Has Spaces", "assets/a"] {
        let refused = f
            .mv("a", "notes", "b.md", None, Some(bad))
            .await
            .unwrap_err();
        assert!(refused.contains("permalink"), "{bad}: {refused}");
    }
    assert!(f.notes().join("a.md").exists(), "nothing moved");
    assert!(!f.notes().join("b.md").exists());
}

#[tokio::test]
async fn an_in_place_rename_keeps_recorded_at_and_records_the_mover() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "projects/velog/alpha.md",
        &engram("Alpha", "velog/alpha", "the alpha body"),
    );
    f.sync().await;

    let receipt = f
        .mv(
            "velog/alpha",
            "notes",
            "projects/velog/alpha.md",
            None,
            Some("path"),
        )
        .await
        .unwrap();
    assert_eq!(receipt["from"]["path"], "projects/velog/alpha.md");
    assert_eq!(receipt["to"]["path"], "projects/velog/alpha.md");
    assert_eq!(receipt["to"]["permalink"], "projects/velog/alpha");
    let text = read(f.notes(), "projects/velog/alpha.md");
    assert!(text.contains("permalink: projects/velog/alpha\n"), "{text}");
    assert!(text.contains("recorded_at: 2026-01-01\n"), "{text}");
    assert!(text.contains("by: mover"), "{text}");
    assert!(text.contains("the alpha body"), "{text}");
    assert!(f.missing("notes", "velog/alpha").await);
    assert!(!f.missing("notes", "projects/velog/alpha").await);
}

#[tokio::test]
async fn a_move_that_changes_nothing_is_refused() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("A", "a", "the a body"));
    f.sync().await;
    let before = read(f.notes(), "a.md");
    for permalink in [None, Some("keep"), Some("path"), Some("a")] {
        let refused = f
            .mv("a", "notes", "a.md", None, permalink)
            .await
            .unwrap_err();
        assert!(
            refused.contains("changes nothing"),
            "{permalink:?}: {refused}"
        );
    }
    assert_eq!(read(f.notes(), "a.md"), before);
}

#[tokio::test]
async fn a_permalink_another_engram_holds_is_refused_naming_it() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("A", "a", "the a body"));
    seed(
        f.notes(),
        "other.md",
        &engram("Other", "taken", "the other body"),
    );
    // Titled like the permalink asked for, which does not hold the address.
    seed(
        f.notes(),
        "decoy.md",
        &engram("free", "decoy", "the decoy body"),
    );
    f.sync().await;

    let refused = f
        .mv("a", "notes", "b.md", None, Some("taken"))
        .await
        .unwrap_err();
    assert!(
        refused.contains("already held") && refused.contains("other.md"),
        "{refused}"
    );
    assert!(f.notes().join("a.md").exists(), "nothing moved");
    assert!(!f.notes().join("b.md").exists());

    f.mv("a", "notes", "b.md", None, Some("free"))
        .await
        .expect("a title is not an address");

    // And across domains: the destination domain's permalinks are the ones
    // that count.
    seed(f.ops(), "b.md", &engram("Ops B", "b", "the ops body"));
    f.sync().await;
    let refused = f
        .mv("free", "notes", "moved.md", Some("ops"), Some("b"))
        .await
        .unwrap_err();
    assert!(refused.contains("already held"), "{refused}");
    assert!(f.notes().join("b.md").exists(), "nothing moved");
}

// --- every reference follows ---------------------------------------------------

const LINKER_BODY: &str = "See [[velog/alpha]] and [[notes:velog/alpha]].\n\
- relates_to [[velog/alpha]]\n\
Anchor: crystalline://notes/velog/alpha#setup\n\
By title: [[Alpha]]\n\
Not this one: [[velog/alpha-two]]\n";

#[tokio::test]
async fn a_permalink_change_rewrites_every_kind_of_reference() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "projects/velog/alpha.md",
        &engram("Alpha", "velog/alpha", "the alpha body"),
    );
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", LINKER_BODY),
    );
    // The longer permalink the linker also names, so that link resolves and
    // the sweep below has nothing of the fixture's own to report.
    seed(
        f.notes(),
        "alpha-two.md",
        &engram("Alpha Two", "velog/alpha-two", "the other one"),
    );
    seed(
        f.ops(),
        "far.md",
        &engram(
            "Far",
            "far",
            "From afar: [[notes:velog/alpha]] and crystalline://notes/velog/alpha.",
        ),
    );
    f.sync().await;

    let receipt = f
        .mv(
            "velog/alpha",
            "notes",
            "projects/velog/alpha.md",
            None,
            Some("path"),
        )
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 2, "{receipt}");
    assert_eq!(receipt["references_rewritten"], 6, "{receipt}");
    let rewritten: Vec<String> = receipt["rewritten"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{}:{}",
                r["domain"].as_str().unwrap(),
                r["permalink"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(rewritten, vec!["notes:linker", "ops:far"], "{receipt}");

    let linker = read(f.notes(), "linker.md");
    assert!(
        linker.contains(
            "See [[projects/velog/alpha]] and [[notes:projects/velog/alpha]].\n\
             - relates_to [[projects/velog/alpha]]\n\
             Anchor: crystalline://notes/projects/velog/alpha#setup\n\
             By title: [[Alpha]]\n\
             Not this one: [[velog/alpha-two]]\n"
        ),
        "{linker}"
    );
    assert!(
        linker.contains("recorded_at: 2026-01-01\n"),
        "a rewrite is not a new recording: {linker}"
    );
    assert!(
        !linker.contains("by: mover"),
        "the mover did not author the linker: {linker}"
    );
    let far = read(f.ops(), "far.md");
    assert!(
        far.contains(
            "From afar: [[notes:projects/velog/alpha]] and crystalline://notes/projects/velog/alpha."
        ),
        "{far}"
    );
    assert!(
        f.unresolved("notes").await.is_empty() && f.unresolved("ops").await.is_empty(),
        "nothing dangles: {:?} {:?}",
        f.unresolved("notes").await,
        f.unresolved("ops").await
    );
}

#[tokio::test]
async fn a_cross_domain_move_rewrites_references_into_a_virtual_domain() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "alpha.md",
        &engram("Alpha", "alpha", "the alpha body"),
    );
    seed(
        f.notes(),
        "linker.md",
        &engram(
            "Linker",
            "linker",
            "[[alpha]], [[Alpha]] and crystalline://notes/alpha\n- relates_to [[alpha]]",
        ),
    );
    f.sync().await;

    let receipt = f
        .mv("alpha", "notes", "archive/alpha.md", Some("vault"), None)
        .await
        .unwrap();
    assert_eq!(receipt["to"]["domain"], "vault");
    assert_eq!(
        receipt["to"]["permalink"], "archive/alpha",
        "in step, so it follows"
    );
    assert_eq!(receipt["references_rewritten"], 4, "{receipt}");
    let linker = read(f.notes(), "linker.md");
    assert!(
        linker.contains(
            "[[vault:archive/alpha]], [[vault:Alpha]] and crystalline://vault/archive/alpha\n\
             - relates_to [[vault:archive/alpha]]"
        ),
        "{linker}"
    );
    assert!(
        f.content("vault", "archive/alpha")
            .await
            .contains("the alpha body")
    );
    assert!(
        f.unresolved("notes").await.is_empty(),
        "{:?}",
        f.unresolved("notes").await
    );
}

#[tokio::test]
async fn virtual_domains_rewrite_at_both_ends() {
    let f = Fixture::new().await;
    for (title, body) in [
        ("Target", "the target body"),
        (
            "Pointer",
            "points at [[target]] and crystalline://vault/target#top",
        ),
    ] {
        f.engine
            .write_engram(&WriteParams {
                domain: "vault".to_string(),
                title: title.to_string(),
                content: body.to_string(),
                folder: None,
                engram_type: None,
                tags: Vec::new(),
                status: None,
                metadata: None,
                overwrite: false,
                share_link: None,
                model: None,
            })
            .await
            .unwrap();
    }

    // Same domain, virtual: a new folder, and the permalink follows.
    let receipt = f
        .mv("target", "vault", "kept/target.md", None, None)
        .await
        .unwrap();
    assert_eq!(receipt["to"]["permalink"], "kept/target");
    assert_eq!(receipt["references_rewritten"], 2, "{receipt}");
    let pointer = f.content("vault", "pointer").await;
    assert!(
        pointer.contains("points at [[kept/target]] and crystalline://vault/kept/target#top"),
        "{pointer}"
    );
    let target = f.content("vault", "kept/target").await;
    assert!(target.contains("permalink: kept/target"), "{target}");

    // Out of the virtual domain into a file one.
    let receipt = f
        .mv(
            "kept/target",
            "vault",
            "target.md",
            Some("notes"),
            Some("keep"),
        )
        .await
        .unwrap();
    assert_eq!(receipt["to"]["permalink"], "kept/target");
    let pointer = f.content("vault", "pointer").await;
    assert!(
        pointer.contains("points at [[notes:kept/target]] and crystalline://notes/kept/target#top"),
        "{pointer}"
    );
    assert!(read(f.notes(), "target.md").contains("permalink: kept/target\n"));
}

#[tokio::test]
async fn update_links_false_leaves_every_reference_as_it_was() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "alpha.md",
        &engram("Alpha", "alpha", "the alpha body"),
    );
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "[[alpha]]"),
    );
    f.sync().await;
    let before = read(f.notes(), "linker.md");
    let receipt = f
        .engine
        .move_engram(
            &MoveParams {
                identifier: "alpha".to_string(),
                domain: "notes".to_string(),
                destination: "guides/alpha.md".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: Some(false),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 0);
    assert_eq!(read(f.notes(), "linker.md"), before);
}

// --- issue 92, end to end ------------------------------------------------------

/// The folder glob the issue's `build_context` call used.
const VELOG: &str = "crystalline://notes/projects/velog/*";

/// Four engrams in `projects/velog/`, three answering to `velog/...` and one
/// to a root permalink, left over from an earlier reorganisation. A glob on
/// the folder misses all four. Each is repaired in place with
/// `permalink: "path"`; afterwards the glob finds all four and every link that
/// pointed at them still resolves.
#[tokio::test]
async fn issue_92_four_drifted_engrams_are_repaired_in_place() {
    let f = Fixture::new().await;
    let drifted = [
        ("projects/velog/intro.md", "Intro", "velog/intro"),
        ("projects/velog/setup.md", "Setup", "velog/setup"),
        ("projects/velog/release.md", "Release", "velog/release"),
        ("projects/velog/overview.md", "Overview", "overview"),
    ];
    for (path, title, permalink) in drifted {
        seed(
            f.notes(),
            path,
            &engram(title, permalink, "- [fact] velog knowledge"),
        );
    }
    seed(
        f.notes(),
        "hub.md",
        &engram(
            "Hub",
            "hub",
            "- relates_to [[velog/intro]]\n- relates_to [[velog/setup]]\n\
             See [[velog/release]], [[overview]] and crystalline://notes/velog/setup#steps.",
        ),
    );
    // One of the four points at a sibling, so a rewrite inside the moved set
    // is covered too.
    seed(
        f.notes(),
        "projects/velog/intro.md",
        &engram(
            "Intro",
            "velog/intro",
            "- [fact] velog knowledge\n- part_of [[overview]]",
        ),
    );
    f.sync().await;

    assert!(
        f.glob(VELOG).await.is_empty(),
        "the glob misses every drifted engram"
    );
    // And the sweep says so before the glob ever has to.
    let drift = f.findings("notes", "V109").await;
    assert_eq!(drift.len(), 4, "{drift:?}");
    assert!(
        drift
            .iter()
            .any(|row| row.contains("permalink=velog/setup; path=projects/velog/setup.md")),
        "{drift:?}"
    );

    for (path, _, permalink) in drifted {
        let receipt = f
            .mv(permalink, "notes", path, None, Some("path"))
            .await
            .unwrap_or_else(|e| panic!("{permalink} repairs in place: {e}"));
        assert_eq!(receipt["to"]["path"], path);
    }

    assert_eq!(
        f.glob(VELOG).await,
        vec![
            "projects/velog/intro",
            "projects/velog/overview",
            "projects/velog/release",
            "projects/velog/setup",
        ]
    );
    let hub = read(f.notes(), "hub.md");
    assert!(
        hub.contains(
            "- relates_to [[projects/velog/intro]]\n- relates_to [[projects/velog/setup]]\n\
             See [[projects/velog/release]], [[projects/velog/overview]] and \
             crystalline://notes/projects/velog/setup#steps."
        ),
        "{hub}"
    );
    assert!(
        read(f.notes(), "projects/velog/intro.md")
            .contains("- part_of [[projects/velog/overview]]")
    );
    assert!(
        f.unresolved("notes").await.is_empty(),
        "every reference still resolves: {:?}",
        f.unresolved("notes").await
    );
    assert!(
        f.findings("notes", "V109").await.is_empty(),
        "and nothing has drifted any more"
    );
    // And a resync changes none of it: the files say what the rows say.
    f.sync().await;
    assert_eq!(f.glob(VELOG).await.len(), 4);
    assert!(f.unresolved("notes").await.is_empty());
}
