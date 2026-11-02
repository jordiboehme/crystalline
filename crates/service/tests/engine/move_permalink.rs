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
                    include_images: false,
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
                    include_images: false,
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

/// A locked domain's references are left as they were: the move rewrites the
/// notes that point at the moved engram and skips `ops/far.md`, because `ops`
/// says `read_only: on`.
#[tokio::test]
async fn a_permalink_change_leaves_a_read_only_domains_references_alone() {
    let f = Fixture::new().await;
    let manifest = crystalline_core::manifest_template("ops", "2026-01-01").replacen(
        "\n---\n",
        "\nread_only: on\n---\n",
        1,
    );
    assert!(manifest.contains("read_only: on"), "{manifest}");
    std::fs::write(f.ops().join("MANIFEST.md"), manifest).unwrap();
    seed(
        f.notes(),
        "projects/velog/alpha.md",
        &engram("Alpha", "velog/alpha", "the alpha body"),
    );
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "See [[velog/alpha]]."),
    );
    seed(
        f.ops(),
        "far.md",
        &engram("Far", "far", "From afar: [[notes:velog/alpha]]."),
    );
    f.sync().await;
    let before = read(f.ops(), "far.md");

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
    assert_eq!(rewritten, vec!["notes:linker"], "{receipt}");
    assert_eq!(
        read(f.ops(), "far.md"),
        before,
        "the locked file is untouched"
    );
    assert!(
        read(f.notes(), "linker.md").contains("[[projects/velog/alpha]]"),
        "while the writable linker followed"
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

// ---------------------------------------------------------------------------
// domain name normalization
// ---------------------------------------------------------------------------

/// A cross-domain move rewrites references to the moved engram's OWN address,
/// but its own text may carry a cross-domain link to some OTHER domain, spelled
/// however the author wrote it - a spelling the move's relink pass never looks
/// at, since that pass follows only this engram's address. It is normalized to
/// the canonical name on the same funnel every other write's final text passes
/// through, moved or not.
#[tokio::test]
async fn a_cross_domain_move_normalizes_the_moved_engrams_own_cross_domain_link() {
    let f = Fixture::new().await;
    // `ops` is registered locally as `ops`; its MANIFEST declares the
    // canonical name `operations`, which nothing else claims here.
    std::fs::write(
        f.ops().join("MANIFEST.md"),
        crystalline_core::manifest_template("operations", "2026-01-01"),
    )
    .unwrap();
    seed(
        f.notes(),
        "alpha.md",
        &engram("Alpha", "alpha", "See [[ops:runbook]] for details."),
    );
    f.sync().await;

    f.mv("alpha", "notes", "alpha.md", Some("vault"), None)
        .await
        .unwrap();

    let moved = f.content("vault", "alpha").await;
    assert!(
        moved.contains("[[operations:runbook]]"),
        "the moved engram's own cross-domain link is normalized to the \
         canonical name, the same as any other write's final text: {moved}"
    );
}

/// A move's relink pass localizes the destination before it ever reaches
/// `relink_engram`, so a referencing engram was written with the destination
/// domain's LOCAL name - not its canonical one, when the two differ. That
/// text passes through the same normalization funnel too.
#[tokio::test]
async fn a_cross_domain_move_writes_referencing_links_with_the_canonical_destination_name() {
    let f = Fixture::new().await;
    // `ops` is registered locally as `ops`; its MANIFEST declares the
    // canonical name `operations`.
    std::fs::write(
        f.ops().join("MANIFEST.md"),
        crystalline_core::manifest_template("operations", "2026-01-01"),
    )
    .unwrap();
    seed(
        f.notes(),
        "alpha.md",
        &engram("Alpha", "alpha", "the alpha body"),
    );
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "See [[alpha]] for details."),
    );
    f.sync().await;

    f.mv("alpha", "notes", "alpha.md", Some("ops"), None)
        .await
        .unwrap();

    let linker = read(f.notes(), "linker.md");
    assert!(
        linker.contains("[[operations:alpha]]"),
        "the rewritten reference carries the destination's canonical name, \
         not its local name 'ops': {linker}"
    );
    assert!(
        f.unresolved("notes").await.is_empty(),
        "and it still resolves: {:?}",
        f.unresolved("notes").await
    );
}

// --- a move and the writers of a linking engram --------------------------------

/// An edit of a linking engram that runs beside a move of the engram it links
/// keeps both changes, whichever of the two reaches the file first.
///
/// The move rewrites the linker under the file's write lock, which the edit
/// takes too, so the two serialize. Two worker threads and a start offset
/// that changes per round, so the rounds do not all take one schedule; the
/// deterministic proof is the engine's
/// `a_relink_waits_for_the_linkers_write_lock`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_during_a_move_is_not_overwritten() {
    for round in 0..20usize {
        let f = Arc::new(Fixture::new().await);
        seed(
            f.notes(),
            "target.md",
            &engram("Target", "target", "the target body"),
        );
        seed(
            f.notes(),
            "linker.md",
            &engram("Linker", "linker", "See [[target]] for details."),
        );
        f.sync().await;

        let edit = EditParams {
            identifier: "linker".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: Some(format!("Edited in round {round}.\n")),
            ..EditParams::default()
        };
        let mover = {
            let f = f.clone();
            tokio::spawn(async move {
                for _ in 0..round % 4 {
                    tokio::task::yield_now().await;
                }
                f.mv("target", "notes", "moved.md", None, None).await
            })
        };
        let editor = {
            let f = f.clone();
            tokio::spawn(async move {
                for _ in 0..(round / 4) % 4 {
                    tokio::task::yield_now().await;
                }
                f.engine.edit_engram(&edit).await.map_err(|e| e.to_string())
            })
        };
        let (moved, edited) = tokio::join!(mover, editor);
        moved
            .unwrap()
            .unwrap_or_else(|e| panic!("round {round}: the move lands: {e}"));
        edited
            .unwrap()
            .unwrap_or_else(|e| panic!("round {round}: the edit lands: {e}"));

        let linker = read(f.notes(), "linker.md");
        assert!(
            linker.contains("See [[moved]] for details."),
            "round {round}: the link follows the move: {linker}"
        );
        assert!(
            linker.contains(&format!("Edited in round {round}.")),
            "round {round}: and the edit is still there: {linker}"
        );
    }
}

/// A virtual linker is rewritten by the store's compare and swap, the way an
/// edit writes it, never last-write-wins over a row that changed under it.
///
/// The seam makes the compare refuse, as a conflict: the move still lands,
/// the linker keeps the text it had rather than taking a rewrite computed
/// from a row that is gone, and the receipt names it.
#[tokio::test]
async fn a_refused_swap_leaves_a_virtual_linker_as_it_was() {
    let f = Fixture::new().await;
    for (title, body) in [
        ("Target", "the target body"),
        ("Pointer", "points at [[target]]"),
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

    f.engine.fail_next_source_edit();
    let receipt = f
        .mv("target", "vault", "kept/target.md", None, None)
        .await
        .expect("the move itself lands");
    assert_eq!(receipt["to"]["permalink"], "kept/target", "{receipt}");
    assert_eq!(receipt["links_rewritten"], 0, "{receipt}");
    let failed = &receipt["links_failed"][0];
    assert_eq!(failed["domain"], "vault", "{receipt}");
    assert_eq!(failed["path"], "pointer.md", "{receipt}");
    assert!(
        failed["reason"].as_str().unwrap().contains("stale edit"),
        "{receipt}"
    );
    let pointer = f.content("vault", "pointer").await;
    assert!(
        pointer.contains("points at [[target]]"),
        "a refused swap writes nothing: {pointer}"
    );
}

/// After a move landed, a linking engram whose references cannot be
/// rewritten is named in the receipt, the way a reflection move names its
/// pages, and the move is not reported as failed. A receipt with nothing to
/// name carries no `links_failed` at all.
#[tokio::test]
async fn an_engram_move_names_a_linker_it_could_not_rewrite() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "target.md",
        &engram("Target", "target", "the target body"),
    );
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "See [[target]] for details."),
    );
    seed(
        f.notes(),
        "other.md",
        &engram("Other", "other", "Also [[target]]."),
    );
    f.sync().await;
    // Not UTF-8 any more: the rewrite cannot read it.
    std::fs::write(
        f.notes().join("linker.md"),
        b"---\ntitle: Linker\n---\n\xff\xfe [[target]]\n",
    )
    .unwrap();

    let receipt = f
        .mv("target", "notes", "moved.md", None, None)
        .await
        .expect("the move itself lands");
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    let failed = receipt["links_failed"].as_array().expect("{receipt}");
    assert_eq!(failed.len(), 1, "{receipt}");
    assert_eq!(failed[0]["domain"], "notes", "{receipt}");
    assert_eq!(failed[0]["path"], "linker.md", "{receipt}");
    assert!(
        failed[0]["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{receipt}"
    );
    assert!(read(f.notes(), "other.md").contains("Also [[moved]]."));

    let receipt = f
        .mv("other", "notes", "elsewhere.md", None, None)
        .await
        .unwrap();
    assert!(receipt.get("links_failed").is_none(), "{receipt}");
}

/// A virtual linker whose swap lost a race with another writer is rewritten
/// again from the row stored now, as an unguarded edit is, and the receipt
/// counts it rewritten.
#[tokio::test]
async fn a_lost_swap_of_a_virtual_linker_is_applied_again() {
    let f = Fixture::new().await;
    for (title, body) in [
        ("Target", "the target body"),
        ("Pointer", "points at [[target]]"),
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

    f.engine.lose_next_relink_swap();
    let receipt = f
        .mv("target", "vault", "kept/target.md", None, None)
        .await
        .expect("the move lands");
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    assert!(receipt.get("links_failed").is_none(), "{receipt}");
    let pointer = f.content("vault", "pointer").await;
    assert!(
        pointer.contains("points at [[kept/target]]"),
        "the second try wrote the rewrite: {pointer}"
    );
}

/// Two moves of engrams that link each other, run at once on two workers,
/// leave neither old file in place and no link to an old address. The
/// deterministic proof is the engine's
/// `crossing_moves_leave_no_link_to_an_old_address`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crossing_moves_leave_no_dangling_link() {
    for round in 0..10usize {
        let f = Arc::new(Fixture::new().await);
        seed(
            f.notes(),
            "alpha.md",
            &engram("Alpha", "alpha", "See [[beta]]."),
        );
        seed(
            f.notes(),
            "beta.md",
            &engram("Beta", "beta", "Back to [[alpha]]."),
        );
        f.sync().await;
        let first = {
            let f = f.clone();
            tokio::spawn(async move {
                for _ in 0..round % 3 {
                    tokio::task::yield_now().await;
                }
                f.mv("alpha", "notes", "gamma.md", None, None).await
            })
        };
        let second = {
            let f = f.clone();
            tokio::spawn(async move { f.mv("beta", "notes", "delta.md", None, None).await })
        };
        let (first, second) = tokio::join!(first, second);
        first
            .unwrap()
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        second
            .unwrap()
            .unwrap_or_else(|e| panic!("round {round}: {e}"));

        assert!(!f.notes().join("alpha.md").exists(), "round {round}");
        assert!(!f.notes().join("beta.md").exists(), "round {round}");
        assert!(
            read(f.notes(), "gamma.md").contains("See [[delta]]."),
            "round {round}: {}",
            read(f.notes(), "gamma.md")
        );
        assert!(
            read(f.notes(), "delta.md").contains("Back to [[gamma]]."),
            "round {round}: {}",
            read(f.notes(), "delta.md")
        );
        assert!(
            f.unresolved("notes").await.is_empty(),
            "round {round}: {:?}",
            f.unresolved("notes").await
        );
    }
}

// --- relative Markdown links ---------------------------------------------------

/// The read receipt of an engram, by address.
async fn read_receipt(f: &Fixture, domain: &str, identifier: &str) -> Value {
    f.engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
                share_link: None,
                include_images: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_or_else(|e| panic!("{domain}:{identifier} should read: {e}"))
}

/// The `(target, resolved, permalink)` of every link and relation an engram
/// reports, relations first.
fn edges(receipt: &Value) -> Vec<(String, bool, Option<String>)> {
    receipt["relations"]
        .as_array()
        .unwrap()
        .iter()
        .chain(receipt["links"].as_array().unwrap())
        .map(|e| {
            (
                e["target"]["target"].as_str().unwrap().to_string(),
                e["resolved"].as_bool().unwrap(),
                e["permalink"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// A relative link names a file, not an address, so it has to follow the file
/// even when the move leaves the permalink where it was: every engram that
/// links the moved one by path is respelled to the new path, from its own
/// folder, with the fragment and the title kept, and the link still resolves.
/// A link inside code is an example and stays as written.
#[tokio::test]
async fn moving_an_engram_respells_relative_links_to_it() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "pumps/b.md",
        &engram("Bee", "kit/bee", "The pump."),
    );
    seed(
        f.notes(),
        "a.md",
        &engram(
            "Ay",
            "ay",
            "- part_of [the pump](pumps/b.md \"The pump\")\n\nRead [b](pumps/b.md#top) first, not `[b](pumps/b.md)`.",
        ),
    );
    seed(
        f.notes(),
        "deep/c.md",
        &engram("Cee", "cee", "Up and over: [b](../pumps/b.md)."),
    );
    // A wikilink spelled as the path binds by path too, from the domain root.
    seed(
        f.notes(),
        "deep/w.md",
        &engram("Dub", "dub", "By path: [[pumps/b.md]]."),
    );
    f.sync().await;

    let receipt = f
        .mv("kit/bee", "notes", "archive/b.md", None, None)
        .await
        .unwrap();
    assert_eq!(receipt["to"]["permalink"], "kit/bee", "{receipt}");
    assert_eq!(receipt["links_rewritten"], 3, "{receipt}");
    assert_eq!(receipt["references_rewritten"], 4, "{receipt}");

    let a = read(f.notes(), "a.md");
    assert!(
        a.contains(
            "- part_of [the pump](archive/b.md \"The pump\")\n\nRead [b](archive/b.md#top) first, not `[b](pumps/b.md)`."
        ),
        "{a}"
    );
    let c = read(f.notes(), "deep/c.md");
    assert!(c.contains("Up and over: [b](../archive/b.md)."), "{c}");
    let w = read(f.notes(), "deep/w.md");
    assert!(w.contains("By path: [[archive/b.md]]."), "{w}");

    let a = read_receipt(&f, "notes", "ay").await;
    assert_eq!(
        edges(&a),
        vec![
            (
                "archive/b.md".to_string(),
                true,
                Some("kit/bee".to_string())
            ),
            (
                "archive/b.md".to_string(),
                true,
                Some("kit/bee".to_string())
            ),
        ],
        "{a}"
    );
    let c = read_receipt(&f, "notes", "cee").await;
    assert_eq!(
        edges(&c),
        vec![(
            "../archive/b.md".to_string(),
            true,
            Some("kit/bee".to_string())
        )],
        "{c}"
    );
    let w = read_receipt(&f, "notes", "dub").await;
    assert_eq!(
        edges(&w)[0],
        (
            "archive/b.md".to_string(),
            true,
            Some("kit/bee".to_string())
        ),
        "{w}"
    );
    assert!(
        f.unresolved("notes").await.is_empty(),
        "{:?}",
        f.unresolved("notes").await
    );
}

/// The moved engram's own relative links were written from its old folder, so
/// they are rebased onto the new one and land on the same files. A link to
/// itself follows it, and what is not a link to another engram's file (an
/// attachment, a web page, an image) is left as written.
#[tokio::test]
async fn moving_an_engram_rebases_its_own_relative_links() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("Ay", "ay", "The root note."));
    seed(
        f.notes(),
        "pumps/peer.md",
        &engram("Peer", "peer", "The pump beside it."),
    );
    seed(
        f.notes(),
        "pumps/b.md",
        &engram(
            "Bee",
            "pumps/b",
            "- part_of [Ay](../a.md#intro)\n\nSee [peer](peer.md \"Peer\"), [me](b.md), \
             ![pic](assets/pic.png), [web](https://x.test/y.md) and `[code](../a.md)`.\n\n\
             Self [[pumps/b.md]] and root [[a.md]].",
        ),
    );
    f.sync().await;

    let receipt = f
        .mv("pumps/b", "notes", "archive/old/b.md", None, None)
        .await
        .unwrap();
    assert_eq!(receipt["to"]["permalink"], "archive/old/b", "{receipt}");

    let b = read(f.notes(), "archive/old/b.md");
    assert!(
        b.contains(
            "- part_of [Ay](../../a.md#intro)\n\nSee [peer](../../pumps/peer.md \"Peer\"), [me](b.md), \
             ![pic](assets/pic.png), [web](https://x.test/y.md) and `[code](../a.md)`.\n\n\
             Self [[archive/old/b.md]] and root [[a.md]]."
        ),
        "{b}"
    );

    let b = read_receipt(&f, "notes", "archive/old/b").await;
    assert_eq!(
        edges(&b),
        vec![
            ("../../a.md".to_string(), true, Some("ay".to_string())),
            (
                "../../pumps/peer.md".to_string(),
                true,
                Some("peer".to_string())
            ),
            ("b.md".to_string(), true, Some("archive/old/b".to_string())),
            (
                "archive/old/b.md".to_string(),
                true,
                Some("archive/old/b".to_string())
            ),
            ("a.md".to_string(), true, Some("ay".to_string())),
        ],
        "{b}"
    );
}

/// A relative path never reaches into another domain, so a move into one
/// must not leave the moved engram's relative links to be read there: a
/// same-path file in the destination would take them over. Each link into
/// the source domain becomes a prefixed wikilink with its text (by permalink
/// where an engram stands there, by path where none does, so it stays
/// unresolved as it was), and a link to the engram itself follows it. A link
/// that climbs out of the source domain but would land on an engram in the
/// destination is named in `links_failed`. The source domain's relative
/// links to the moved engram name its new address the same way.
#[tokio::test]
async fn a_cross_domain_move_turns_relative_links_into_addresses() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "guides/a.md",
        &engram(
            "Ay",
            "guides/a",
            "- part_of [Bee](b.md)\n\nSee [B](b.md), [me](a.md), [gone](c.md), \
             [up](index.md) and [out](../../out.md).",
        ),
    );
    seed(
        f.notes(),
        "guides/b.md",
        &engram("Bee", "guides/b", "Back to [A](a.md) and [[guides/a.md]]."),
    );
    // The decoys: the same paths seen from the destination, other engrams.
    seed(
        f.ops(),
        "x/y/b.md",
        &engram("Other B", "x/y/other-b", "Not the one."),
    );
    seed(
        f.ops(),
        "x/y/c.md",
        &engram("Other C", "x/y/other-c", "Not the one either."),
    );
    seed(f.ops(), "out.md", &engram("Out", "out", "Outside."));
    f.sync().await;

    let receipt = f
        .mv("guides/a", "notes", "x/y/a.md", Some("ops"), None)
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    assert_eq!(receipt["references_rewritten"], 2, "{receipt}");
    // The link to a file the source never had was dead before the move and
    // is named; the folder's index.md is no engram link at all.
    assert_eq!(
        dead(&receipt),
        vec![(
            "[[notes:guides/c.md]]".to_string(),
            "unresolved_before".to_string()
        )],
        "{receipt}"
    );
    let failed = receipt["links_failed"].as_array().expect("links_failed");
    assert_eq!(failed.len(), 1, "{receipt}");
    assert_eq!(failed[0]["domain"], "ops", "{receipt}");
    assert_eq!(failed[0]["path"], "x/y/a.md", "{receipt}");
    assert!(
        failed[0]["reason"]
            .as_str()
            .unwrap()
            .contains("../../out.md"),
        "{receipt}"
    );

    let a = read(f.ops(), "x/y/a.md");
    assert!(
        a.contains(
            "- part_of [[notes:guides/b|Bee]]\n\nSee [[notes:guides/b|B]], [me](a.md), \
             [[notes:guides/c.md|gone]], [up](index.md) and [out](../../out.md)."
        ),
        "{a}"
    );
    let b = read(f.notes(), "guides/b.md");
    assert!(
        b.contains("Back to [[ops:x/y/a|A]] and [[ops:x/y/a]]."),
        "{b}"
    );

    let a = read_receipt(&f, "ops", "x/y/a").await;
    let bound = edges(&a);
    assert!(bound.contains(&("guides/b".to_string(), true, None)), "{a}");
    assert!(
        bound.contains(&("guides/c.md".to_string(), false, None)),
        "{a}"
    );
    assert!(
        !bound
            .iter()
            .any(|(_, _, p)| matches!(p.as_deref(), Some("x/y/other-b" | "x/y/other-c"))),
        "no link binds to a decoy: {a}"
    );
    let b = read_receipt(&f, "notes", "guides/b").await;
    assert_eq!(
        edges(&b),
        vec![("x/y/a".to_string(), true, None)],
        "one target, read once per line: {b}"
    );
}

/// A bare wikilink spelled as a path the source domain has no file at is
/// read from the domain root, so after a move into another domain it would
/// bind to a same-path file there: it names the source domain instead and
/// stays unresolved, as it was.
#[tokio::test]
async fn a_cross_domain_move_gives_a_missing_path_wikilink_its_domain() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "guides/a.md",
        &engram("Ay", "guides/a", "See [[guides/missing.md|M]]."),
    );
    seed(
        f.ops(),
        "guides/missing.md",
        &engram("Decoy", "guides/decoy", "Not the one."),
    );
    f.sync().await;

    let receipt = f
        .mv("guides/a", "notes", "guides/a.md", Some("ops"), None)
        .await
        .unwrap();
    assert!(receipt.get("links_failed").is_none(), "{receipt}");
    let a = read(f.ops(), "guides/a.md");
    assert!(a.contains("See [[notes:guides/missing.md|M]]."), "{a}");
    let a = read_receipt(&f, "ops", "guides/a").await;
    assert_eq!(
        edges(&a),
        vec![("guides/missing.md".to_string(), false, None)],
        "{a}"
    );
}

/// A wikilink from another domain spelled as the moved engram's path,
/// `[[notes:guides/a.md]]`, binds by path and follows the file like every
/// other form: to its new path inside its domain, and to its new address
/// when it leaves for another domain.
#[tokio::test]
async fn a_prefixed_path_wikilink_follows_a_move() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "guides/a.md",
        &engram("Ay", "guides/a", "The engram."),
    );
    seed(
        f.ops(),
        "linker.md",
        &engram("Linker", "linker", "See [[notes:guides/a.md|A]]."),
    );
    f.sync().await;

    let receipt = f
        .mv("guides/a", "notes", "archive/a.md", None, Some("keep"))
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    let linker = read(f.ops(), "linker.md");
    assert!(linker.contains("See [[notes:archive/a.md|A]]."), "{linker}");
    let l = read_receipt(&f, "ops", "linker").await;
    assert!(edges(&l)[0].1, "{l}");

    let receipt = f
        .mv("guides/a", "notes", "x/a.md", Some("vault"), None)
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    let linker = read(f.ops(), "linker.md");
    assert!(linker.contains("See [[vault:x/a.md|A]]."), "{linker}");
    let l = read_receipt(&f, "ops", "linker").await;
    assert!(edges(&l)[0].1, "{l}");
}

// --- issue 152: the moved engram's own bare links ------------------------------

/// The `link` and `reason` of every entry in a move receipt's `links_dead`,
/// empty when the receipt carries none.
fn dead(receipt: &Value) -> Vec<(String, String)> {
    receipt["links_dead"]
        .as_array()
        .map(|dead| {
            dead.iter()
                .map(|d| {
                    (
                        d["link"].as_str().unwrap().to_string(),
                        d["reason"].as_str().unwrap().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The issue's repro: `a` links `[[b]]` and `[[c]]`, both in `notes`, and
/// moves to `ops`. In `ops` a bare link resolves in `ops`, so both gain the
/// domain they were written in and still resolve.
#[tokio::test]
async fn issue_152_a_moved_engrams_bare_links_keep_resolving() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[b]] and [[c]]."),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    seed(f.notes(), "c.md", &engram("Cee", "c", "C."));
    f.sync().await;

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[notes:b]] and [[notes:c]]."), "{a}");
    assert!(dead(&receipt).is_empty(), "{receipt}");
    let a = read_receipt(&f, "ops", "a").await;
    assert!(edges(&a).iter().all(|(_, resolved, _)| *resolved), "{a}");
    assert!(
        f.unresolved("ops").await.is_empty(),
        "{:?}",
        f.unresolved("ops").await
    );
}

/// Relation bullets are written in the same brackets and follow the same
/// rule. A display text stays as written; a link by title names the target
/// by its permalink, so it survives a later change of that title, and keeps
/// the author's words as its display text.
#[tokio::test]
async fn a_cross_domain_move_prefixes_relations_aliases_and_title_links() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "process/a.md",
        &engram(
            "Ay",
            "process/a",
            "- relates_to [[process/b]]\n- part_of [[process/b|the bee]]\n\n\
             See [[process/b|B]], [[Cee Title]], [[cee title|the cee]] and `[[process/b]]`.",
        ),
    );
    seed(f.notes(), "process/b.md", &engram("Bee", "process/b", "B."));
    seed(
        f.notes(),
        "sources/c.md",
        &engram("Cee Title", "sources/c", "C."),
    );
    f.sync().await;

    let receipt = f
        .mv("process/a", "notes", "process/a.md", Some("ops"), None)
        .await
        .unwrap();
    let a = read(f.ops(), "process/a.md");
    assert!(
        a.contains(
            "- relates_to [[notes:process/b]]\n- part_of [[notes:process/b|the bee]]\n\n\
             See [[notes:process/b|B]], [[notes:sources/c|Cee Title]], \
             [[notes:sources/c|the cee]] and `[[process/b]]`."
        ),
        "{a}"
    );
    assert!(dead(&receipt).is_empty(), "{receipt}");
    let a = read_receipt(&f, "ops", "process/a").await;
    assert!(edges(&a).iter().all(|(_, resolved, _)| *resolved), "{a}");
}

/// A target moved into `ops` first: its inbound rewrite gave the linker
/// `[[ops:b]]` while the linker still sat in `notes`. When the linker
/// follows, that prefix names its own domain and collapses to `[[b]]`, and a
/// bare link that names an engram in the destination and nothing in the
/// source stays bare.
#[tokio::test]
async fn a_target_moved_first_leaves_the_linker_bare_once_it_follows() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[b]] and [[d|the dee]]."),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    seed(f.ops(), "d.md", &engram("Dee", "d", "Already here."));
    f.sync().await;

    f.mv("b", "notes", "b.md", Some("ops"), None).await.unwrap();
    let a = read(f.notes(), "a.md");
    assert!(a.contains("See [[ops:b]] and [[d|the dee]]."), "{a}");

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[b]] and [[d|the dee]]."), "{a}");
    assert!(dead(&receipt).is_empty(), "{receipt}");
    let a = read_receipt(&f, "ops", "a").await;
    assert!(edges(&a).iter().all(|(_, resolved, _)| *resolved), "{a}");
}

/// A target moved into `ops` after the linker, under a new slug: the
/// linker's `[[notes:b]]` is an inbound reference by then, so the second move
/// rewrites it, and in an engram of `ops` the prefix collapses.
#[tokio::test]
async fn a_target_moved_later_with_a_new_slug_is_followed_and_collapses() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[b|the bee]].\n- relates_to [[b]]"),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    f.sync().await;

    f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(
        a.contains("See [[notes:b|the bee]].\n- relates_to [[notes:b]]"),
        "{a}"
    );

    let receipt = f
        .mv("b", "notes", "bee.md", Some("ops"), Some("bee"))
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    let a = read(f.ops(), "a.md");
    assert!(
        a.contains("See [[bee|the bee]].\n- relates_to [[bee]]"),
        "{a}"
    );
    let a = read_receipt(&f, "ops", "a").await;
    assert!(edges(&a).iter().all(|(_, resolved, _)| *resolved), "{a}");
}

/// A link to the moved engram itself stays bare and follows its new
/// permalink, even when that new permalink is what another engram of the
/// source domain answers to: the other engram's link gains the prefix, the
/// self-link does not.
#[tokio::test]
async fn a_link_to_the_moved_engram_itself_stays_bare() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "Me: [[a]], by title [[Ay]]; the other: [[a2]]."),
    );
    seed(f.notes(), "a2.md", &engram("Other", "a2", "Not the one."));
    f.sync().await;

    let receipt = f
        .mv("a", "notes", "a2.md", Some("ops"), Some("a2"))
        .await
        .unwrap();
    let a = read(f.ops(), "a2.md");
    assert!(
        a.contains("Me: [[a2]], by title [[Ay]]; the other: [[notes:a2]]."),
        "{a}"
    );
    assert!(dead(&receipt).is_empty(), "{receipt}");
}

/// A link already carrying a prefix of a third domain is left alone, and so
/// is one to the source domain: both resolve where they did.
#[tokio::test]
async fn an_already_prefixed_link_is_untouched() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[vault:x]] and [[notes:b]]."),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    f.sync().await;
    let x: WriteParams = serde_json::from_value(serde_json::json!({
        "domain": "vault", "title": "x", "content": "X."
    }))
    .unwrap();
    f.engine.write_engram(&x).await.unwrap();
    assert!(
        edges(&read_receipt(&f, "notes", "a").await)
            .iter()
            .all(|(_, resolved, _)| *resolved)
    );

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[vault:x]] and [[notes:b]]."), "{a}");
    assert!(dead(&receipt).is_empty(), "{receipt}");
}

/// A link that resolved nowhere before the move stays as written, and the
/// receipt names it in `links_dead` so the caller sees it at once. A
/// `#heading` after a target is a fragment, so `[[b#setup]]` resolves to `b`
/// like `[[b]]` beside it, and both gain their prefix, the fragment kept.
#[tokio::test]
async fn a_link_that_never_resolved_stays_and_is_named_in_links_dead() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[ghost]], [[b#setup]] and [[b]]."),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    f.sync().await;
    let before = edges(&read_receipt(&f, "notes", "a").await);
    assert!(
        before.contains(&("b".to_string(), true, None)),
        "the fragment is not part of the target: {before:?}"
    );

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(
        a.contains("See [[ghost]], [[notes:b#setup]] and [[notes:b]]."),
        "{a}"
    );
    assert_eq!(
        dead(&receipt),
        vec![("[[ghost]]".to_string(), "unresolved_before".to_string())],
        "{receipt}"
    );
    assert!(receipt.get("links_failed").is_none(), "{receipt}");
}

/// A wikilink to a heading of a missing engram is named whole, with its
/// heading, in `links_dead`.
#[tokio::test]
async fn a_dead_heading_link_is_named_with_its_heading() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("Ay", "a", "See [[ghost#x]]."));
    f.sync().await;

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    assert_eq!(
        dead(&receipt),
        vec![("[[ghost#x]]".to_string(), "unresolved_before".to_string())],
        "{receipt}"
    );
    assert!(read(f.ops(), "a.md").contains("See [[ghost#x]]."));
}

/// A permalink rename rewrites `[[b#setup|t]]` in a linking engram to the new
/// permalink and keeps the heading and the text; the receipt counts it.
#[tokio::test]
async fn a_renamed_permalink_rewrites_a_heading_link() {
    let f = Fixture::new().await;
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "See [[b#setup|t]]."),
    );
    f.sync().await;

    let receipt = f
        .mv("b", "notes", "b.md", None, Some("newb"))
        .await
        .unwrap();
    assert_eq!(receipt["links_rewritten"], 1, "{receipt}");
    assert_eq!(receipt["references_rewritten"], 1, "{receipt}");
    let linker = read(f.notes(), "linker.md");
    assert!(linker.contains("See [[newb#setup|t]]."), "{linker}");
    let edges = edges(&read_receipt(&f, "notes", "linker").await);
    assert!(!edges.is_empty(), "the linker has no edges");
    assert!(edges.iter().all(|(_, resolved, _)| *resolved), "{edges:?}");
}

/// A relative link with an anchor becomes a wikilink with the domain and keeps
/// the anchor when its target moves to another domain, with nothing failed.
#[tokio::test]
async fn a_cross_domain_move_keeps_the_anchor_of_a_relative_link() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [p](p.md#setup)."),
    );
    seed(f.notes(), "p.md", &engram("Pee", "p", "P."));
    f.sync().await;

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    assert!(receipt.get("links_failed").is_none(), "{receipt}");
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[notes:p#setup|p]]."), "{a}");
}

/// A link by a title that holds a `#`, in an engram beside one titled `C`,
/// is prefixed to the permalink of the `C# Basics` engram on a cross-domain
/// move: the whole text is the title, not `C` with a heading.
#[tokio::test]
async fn a_cross_domain_move_resolves_a_hash_title_whole() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("Ay", "a", "See [[C# Basics]]."));
    seed(f.notes(), "c.md", &engram("C", "c", "C."));
    seed(
        f.notes(),
        "c-basics.md",
        &engram("C# Basics", "c-basics", "Basics."),
    );
    f.sync().await;

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[notes:c-basics|C# Basics]]."), "{a}");
    assert!(dead(&receipt).is_empty(), "{receipt}");
}

/// `read_engram` reports a heading link with its fragment, resolved.
#[tokio::test]
async fn read_engram_reports_the_fragment_of_a_heading_link() {
    let f = Fixture::new().await;
    seed(f.notes(), "a.md", &engram("Ay", "a", "See [[b#setup]]."));
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    f.sync().await;

    let a = read_receipt(&f, "notes", "a").await;
    let link = &a["links"].as_array().unwrap()[0];
    assert_eq!(
        link["target"],
        serde_json::json!({"target": "b", "fragment": "setup", "domain": null}),
        "{a}"
    );
    assert_eq!(link["resolved"], true, "{a}");
}

/// A title link keeps its words and its fragment when a move names the
/// permalink, and a title that holds a `#` is read whole, so it is not split
/// into a target and a heading.
#[tokio::test]
async fn a_title_link_with_a_fragment_or_a_hash_follows_a_move() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram(
            "Ay",
            "a",
            "See [[Bee#setup]], [[C# Basics]] and [[c#intro]].",
        ),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    seed(f.notes(), "c.md", &engram("C", "c", "C."));
    seed(
        f.notes(),
        "c-basics.md",
        &engram("C# Basics", "c-basics", "Basics."),
    );
    f.sync().await;
    assert!(
        edges(&read_receipt(&f, "notes", "a").await)
            .iter()
            .all(|(_, resolved, _)| *resolved)
    );

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(
        a.contains(
            "See [[notes:b#setup|Bee]], [[notes:c-basics|C# Basics]] and [[notes:c#intro]]."
        ),
        "{a}"
    );
    assert!(dead(&receipt).is_empty(), "{receipt}");
}

/// A title that holds a `#` names the engram that carries it, never the one
/// its split target names. Moving `c` (title `C`) leaves a linker's
/// `[[C# Basics]]` alone while `[[C]]` beside it follows; moving `c-basics`
/// leaves its own `[[C# Basics]]` a self-reference rather than a link to `c`.
#[tokio::test]
async fn a_title_holding_a_hash_is_never_read_as_its_split_target_by_a_move() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "See [[C]] and [[C# Basics]]."),
    );
    seed(f.notes(), "c.md", &engram("C", "c", "C."));
    seed(
        f.notes(),
        "c-basics.md",
        &engram("C# Basics", "c-basics", "Itself: [[C# Basics]]."),
    );
    f.sync().await;

    f.mv("c", "notes", "c.md", Some("ops"), None).await.unwrap();
    let linker = read(f.notes(), "linker.md");
    assert!(
        linker.contains("See [[ops:C]] and [[C# Basics]]."),
        "{linker}"
    );

    f.mv("c-basics", "notes", "c-basics.md", Some("ops"), None)
        .await
        .unwrap();
    let moved = read(f.ops(), "c-basics.md");
    assert!(moved.contains("Itself: [[C# Basics]]."), "{moved}");
    let linker = read(f.notes(), "linker.md");
    assert!(
        linker.contains("See [[ops:C]] and [[ops:C# Basics]]."),
        "{linker}"
    );
}

/// An engram titled `C# Basics`, written after a linker's `[[C# Basics]]`
/// was bound to `C` through its split target, takes the link over: the
/// index then answers what core's resolver answers, read from both ends.
#[tokio::test]
async fn a_later_hash_title_takes_over_a_bound_link() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "linker.md",
        &engram("Linker", "linker", "See [[C# Basics]]."),
    );
    seed(f.notes(), "c.md", &engram("C", "c", "C."));
    f.sync().await;
    let inbound = |v: &Value| v["inbound"]["count"].as_u64().unwrap_or(0);
    assert_eq!(inbound(&read_receipt(&f, "notes", "c").await), 1);

    let write: WriteParams = serde_json::from_value(serde_json::json!({
        "domain": "notes", "title": "C# Basics", "content": "Basics."
    }))
    .unwrap();
    let written = f.engine.write_engram(&write).await.unwrap();
    let permalink = written["permalink"]
        .as_str()
        .unwrap_or("c-basics")
        .to_string();
    assert_eq!(inbound(&read_receipt(&f, "notes", "c").await), 0);
    assert_eq!(inbound(&read_receipt(&f, "notes", &permalink).await), 1);
    assert!(
        edges(&read_receipt(&f, "notes", "linker").await)
            .iter()
            .all(|(_, resolved, _)| *resolved)
    );
}

/// A move inside one domain leaves bare links bare and carries no
/// `links_dead`, and `update_links: false` skips the cross-domain rewrite
/// of the moved engram's own links as it skips every other.
#[tokio::test]
async fn same_domain_moves_and_update_links_false_leave_bare_links_alone() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[b]] and [[ghost]]."),
    );
    seed(f.notes(), "b.md", &engram("Bee", "b", "B."));
    f.sync().await;

    let receipt = f
        .mv("a", "notes", "guides/a.md", None, Some("keep"))
        .await
        .unwrap();
    let a = read(f.notes(), "guides/a.md");
    assert!(a.contains("See [[b]] and [[ghost]]."), "{a}");
    assert!(receipt.get("links_dead").is_none(), "{receipt}");

    let receipt = f
        .engine
        .move_engram(
            &MoveParams {
                identifier: "a".to_string(),
                domain: "notes".to_string(),
                destination: "a.md".to_string(),
                destination_domain: Some("ops".to_string()),
                permalink: None,
                update_links: Some(false),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[b]] and [[ghost]]."), "{a}");
    assert!(receipt.get("links_dead").is_none(), "{receipt}");
}

/// A title whose head reads like a domain prefix (`[[Log: Weekly]]`) is no
/// prefix when no domain answers to it: the resolver reads the whole text at
/// home, so the move treats it as the bare link it is.
#[tokio::test]
async fn a_colon_title_link_is_a_bare_link_and_gains_the_prefix() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "a.md",
        &engram("Ay", "a", "See [[Log: Weekly]]."),
    );
    seed(
        f.notes(),
        "weekly.md",
        &engram("Log: Weekly", "weekly", "The log.")
            .replace("title: Log: Weekly", "title: \"Log: Weekly\""),
    );
    f.sync().await;
    let r = read_receipt(&f, "notes", "a").await;
    assert!(edges(&r)[0].1, "{r}");

    let receipt = f.mv("a", "notes", "a.md", Some("ops"), None).await.unwrap();
    let a = read(f.ops(), "a.md");
    assert!(a.contains("See [[notes:weekly|Log: Weekly]]."), "{a}");
    assert!(dead(&receipt).is_empty(), "{receipt}");
}

/// An engram of the destination that names the moved engram by its path
/// with the source's prefix (`[[notes:guides/a.md]]`) follows it, and the
/// prefix that would name its own domain is dropped.
#[tokio::test]
async fn a_prefixed_path_wikilink_in_the_destination_collapses() {
    let f = Fixture::new().await;
    seed(
        f.notes(),
        "guides/a.md",
        &engram("Ay", "guides/a", "The engram."),
    );
    seed(
        f.ops(),
        "linker.md",
        &engram(
            "Linker",
            "linker",
            "See [[notes:guides/a.md|A]] and [[notes:guides/a]].",
        ),
    );
    f.sync().await;

    f.mv("guides/a", "notes", "x/a.md", Some("ops"), None)
        .await
        .unwrap();
    let linker = read(f.ops(), "linker.md");
    assert!(linker.contains("See [[x/a.md|A]] and [[x/a]]."), "{linker}");
    let l = read_receipt(&f, "ops", "linker").await;
    assert!(edges(&l).iter().all(|(_, resolved, _)| *resolved), "{l}");
}
