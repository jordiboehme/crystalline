//! Issue 112: an overwrite replaces the engram that owns the permalink in its
//! own file, whatever that file is called, and no write leaves a stray file.

use std::path::Path;
use std::sync::Arc;

use crate::support;
use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::TursoStore;
use crystalline_service::changes::{Change, ChangeKind, Envelope};
use crystalline_service::params::*;
use crystalline_service::{Engine, Scope};
use tokio::sync::{Mutex, broadcast};

const OWNER: &str = "conventions/Code Review Standards.md";
const SLUG: &str = "conventions/code-review-standards.md";
const PERMALINK: &str = "conventions/code-review-standards";

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

fn seed(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A file domain `notes` holding a MANIFEST and whatever `files` seeds,
/// synced; `review` puts it in review mode.
async fn fixture(
    review: bool,
    files: &[(&str, String)],
) -> (tempfile::TempDir, Arc<Engine>, support::ScratchStateDir) {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let notes = tmp.path().join("notes");
    seed(
        &notes,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: notes\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# notes\n\n## Scope\n\n- Everything about notes\n\n## When to Use\n\n- Route here for notes questions\n",
    );
    for (rel, text) in files {
        seed(&notes, rel, text);
    }
    let mut entry = DomainEntry::file(notes);
    if review {
        entry.review = Some(ReviewMode::Overlay);
    }
    let mut cfg = GlobalConfig::default();
    cfg.domains.insert("notes".to_string(), entry);
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(tmp.path().join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
}

fn capture(title: &str, folder: Option<&str>, content: &str, overwrite: bool) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: folder.map(str::to_string),
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite,
        share_link: None,
        model: None,
    }
}

fn read_as(identifier: &str) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: Some("notes".to_string()),
        share_link: None,
    }
}

fn drain(rx: &mut broadcast::Receiver<Envelope>) -> Vec<Change> {
    let mut out = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        out.push(envelope.change);
    }
    out
}

#[tokio::test]
async fn an_overwrite_replaces_a_title_named_file_in_place() {
    let (tmp, engine, _scratch) = fixture(
        false,
        &[(
            OWNER,
            engram("Code Review Standards", PERMALINK, "The old rule."),
        )],
    )
    .await;
    let notes = tmp.path().join("notes");
    let mut rx = engine.changes().subscribe();

    let receipt = engine
        .write_engram(&capture(
            "Code Review Standards",
            Some("conventions"),
            "The new rule.",
            true,
        ))
        .await
        .unwrap();

    assert_eq!(receipt["path"], OWNER, "{receipt}");
    assert_eq!(receipt["permalink"], PERMALINK);
    assert_eq!(receipt["action"], "written");
    assert!(
        !notes.join(SLUG).exists(),
        "no second file at the slug path"
    );
    let text = std::fs::read_to_string(notes.join(OWNER)).unwrap();
    assert!(
        text.contains("The new rule.") && !text.contains("The old rule."),
        "{text}"
    );
    assert!(text.contains(&format!("permalink: {PERMALINK}")), "{text}");

    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let Change::Engram(change) = &heard[0] else {
        panic!("{heard:?}")
    };
    assert_eq!(
        (change.kind, change.path.as_str()),
        (ChangeKind::Modified, OWNER)
    );

    // The watcher's pass behind the write: nothing moved, nothing failed.
    let report = engine
        .sync_paths("notes", vec![OWNER.to_string(), SLUG.to_string()])
        .await
        .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        (report.added, report.updated, report.deleted, report.moved),
        (0, 0, 0, 0),
        "the engine recorded the stamp, so the pass reads it unchanged: {report:?}"
    );
    assert!(drain(&mut rx).is_empty(), "the watcher announces nothing");

    let read = engine
        .read_engram(&read_as(PERMALINK), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(read["content"].as_str().unwrap().contains("The new rule."));
}

#[tokio::test]
async fn an_overwrite_matches_the_folder_by_slug() {
    let owner = "Conventions/Code Review Standards.md";
    let (tmp, engine, _scratch) = fixture(
        false,
        &[(
            owner,
            engram("Code Review Standards", PERMALINK, "The old rule."),
        )],
    )
    .await;
    let receipt = engine
        .write_engram(&capture(
            "Code Review Standards",
            Some("conventions"),
            "The new rule.",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(receipt["path"], owner);
    let notes = tmp.path().join("notes");
    assert!(
        std::fs::read_to_string(notes.join(owner))
            .unwrap()
            .contains("The new rule.")
    );
}

#[tokio::test]
async fn an_overwrite_of_an_engram_in_another_folder_is_refused_before_disk() {
    let moved = "archive/code-review-standards.md";
    let old = engram("Code Review Standards", PERMALINK, "The old rule.");
    // A root engram moved into a folder with its permalink kept.
    let root_moved = engram("Root Rule", "root-rule", "Kept its root permalink.");
    let (tmp, engine, _scratch) = fixture(
        false,
        &[(moved, old.clone()), ("archive/root-rule.md", root_moved)],
    )
    .await;
    let err = engine
        .write_engram(&capture(
            "Code Review Standards",
            Some("conventions"),
            "The new rule.",
            true,
        ))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "permalink 'conventions/code-review-standards' in domain 'notes' belongs to 'archive/code-review-standards.md' in folder 'archive', not in folder 'conventions'. An overwrite replaces an engram where it lives: move it with move_engram first, or change it in place with edit_engram"
    );
    assert!(
        !err.contains("already exists in domain"),
        "never the elicitation marker"
    );
    let notes = tmp.path().join("notes");
    assert!(
        !notes.join(SLUG).exists(),
        "nothing written at the slug path"
    );
    assert_eq!(std::fs::read_to_string(notes.join(moved)).unwrap(), old);

    // The root is named as such.
    let err = engine
        .write_engram(&capture("Root Rule", None, "x", true))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "permalink 'root-rule' in domain 'notes' belongs to 'archive/root-rule.md' in folder 'archive', not in the domain root. An overwrite replaces an engram where it lives: move it with move_engram first, or change it in place with edit_engram"
    );
    assert!(!notes.join("root-rule.md").exists());
}

#[tokio::test]
async fn a_write_never_replaces_a_file_that_holds_another_engram() {
    let other = engram("Something Else", "custom-gamma", "Not gamma.");
    let (tmp, engine, _scratch) = fixture(false, &[("gamma.md", other.clone())]).await;
    for overwrite in [false, true] {
        let err = engine
            .write_engram(&capture("Gamma", None, "Gamma body.", overwrite))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "'gamma.md' in domain 'notes' already holds the engram 'custom-gamma'. Pick another title or folder, or change 'custom-gamma' with edit_engram",
            "overwrite={overwrite}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("notes/gamma.md")).unwrap(),
        other
    );
}

#[tokio::test]
async fn a_create_refuses_an_unreadable_file_and_an_overwrite_repairs_it() {
    let broken = "---\ntitle: [unclosed\n---\n\nbody\n".to_string();
    let (tmp, engine, _scratch) = fixture(false, &[("gamma.md", broken.clone())]).await;
    let err = engine
        .write_engram(&capture("Gamma", None, "Gamma body.", false))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "'gamma.md' in domain 'notes' already holds a file that is not a readable engram. Pick another title or folder, or pass overwrite=true to replace it"
    );
    let path = tmp.path().join("notes/gamma.md");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);

    engine
        .write_engram(&capture("Gamma", None, "Gamma body.", true))
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("Gamma body.")
    );
}

#[tokio::test]
async fn a_title_namesake_is_not_an_owner() {
    let namesake = engram("Gamma", "g", "Answers to g, titled Gamma.");
    let (tmp, engine, _scratch) = fixture(false, &[("g.md", namesake.clone())]).await;
    let receipt = engine
        .write_engram(&capture("Gamma", None, "The real gamma.", true))
        .await
        .unwrap();
    assert_eq!(receipt["path"], "gamma.md");
    let notes = tmp.path().join("notes");
    assert_eq!(
        std::fs::read_to_string(notes.join("g.md")).unwrap(),
        namesake
    );
}

fn ada() -> Scope {
    Scope::User {
        account: "ada".to_string(),
        admin: false,
    }
}

/// Bob, working inside ada's draft of the title-named page.
fn bob_in_adas_draft() -> (Scope, crystalline_service::Join) {
    (
        Scope::User {
            account: "bob".to_string(),
            admin: false,
        },
        crystalline_service::Join {
            account: "bob".to_string(),
            holder: crystalline_service::Holder::Process(1),
            domain: "notes".to_string(),
            path: OWNER.to_string(),
            owner: "ada".to_string(),
            expires_at: None,
        },
    )
}

#[tokio::test]
async fn a_joined_overwrite_of_the_granted_title_named_page_lands_in_the_owners_draft() {
    let (_tmp, engine, _scratch) = fixture(
        true,
        &[(
            OWNER,
            engram("Code Review Standards", PERMALINK, "The old rule."),
        )],
    )
    .await;
    engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Ada's rule.",
                true,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap();
    let (bob, join) = bob_in_adas_draft();
    let receipt = engine
        .write_engram_joined(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Bob's rule.",
                true,
            ),
            None,
            &bob,
            Some(&join),
        )
        .await
        .unwrap();
    assert_eq!(receipt["path"], OWNER);
    assert_eq!(receipt["joined"], "landed in ada's draft");
    let hers = engine
        .read_engram(&read_as(PERMALINK), &ada())
        .await
        .unwrap();
    assert!(hers["content"].as_str().unwrap().contains("Bob's rule."));
}

#[tokio::test]
async fn a_joined_capture_elsewhere_hears_the_join_refusal_before_anything_about_the_draft() {
    let (_tmp, engine, _scratch) = fixture(
        true,
        &[
            (
                OWNER,
                engram("Code Review Standards", PERMALINK, "The old rule."),
            ),
            (
                "archive/root-rule.md",
                engram("Root Rule", "root-rule", "Moved."),
            ),
        ],
    )
    .await;
    engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Ada's rule.",
                true,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap();
    let (bob, join) = bob_in_adas_draft();
    // Would be M1 for anybody else; inside a join it is the join refusal.
    let err = engine
        .write_engram_joined(
            &capture("Root Rule", None, "x", true),
            None,
            &bob,
            Some(&join),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("is working inside ada's draft"), "{err}");
    assert!(!err.contains("belongs to"), "{err}");
}

#[tokio::test]
async fn an_overwrite_in_review_mode_drafts_the_owning_path() {
    let old = engram("Code Review Standards", PERMALINK, "The old rule.");
    let (tmp, engine, _scratch) = fixture(true, &[(OWNER, old.clone())]).await;
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    let receipt = engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "The new rule.",
                true,
            ),
            None,
            &ada,
        )
        .await
        .unwrap();
    assert_eq!(receipt["path"], OWNER, "{receipt}");
    assert_eq!(receipt["draft"], true);

    let notes = tmp.path().join("notes");
    assert_eq!(
        std::fs::read_to_string(notes.join(OWNER)).unwrap(),
        old,
        "the folder is untouched"
    );
    assert!(!notes.join(SLUG).exists());

    let mine = engine.read_engram(&read_as(PERMALINK), &ada).await.unwrap();
    assert_eq!(mine["path"], OWNER);
    assert!(mine["content"].as_str().unwrap().contains("The new rule."));
    let team = engine
        .read_engram(&read_as(PERMALINK), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(team["content"].as_str().unwrap().contains("The old rule."));
}

#[tokio::test]
async fn a_joined_refusal_names_the_path_the_caller_built_never_a_file_in_the_owners_draft() {
    let (_tmp, engine, _scratch) = fixture(
        true,
        &[(
            OWNER,
            engram("Code Review Standards", PERMALINK, "The old rule."),
        )],
    )
    .await;
    // Ada drafts the granted page, and privately a plan she keeps under a
    // file name of her own that no base file has.
    engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Ada's rule.",
                true,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap();
    engine
        .write_engram_as(
            &capture("Secret Plan", Some("conventions"), "Ada's plan.", false),
            None,
            &ada(),
        )
        .await
        .unwrap();
    engine
        .move_engram_as(
            &MoveParams {
                identifier: "conventions/secret-plan".to_string(),
                domain: "notes".to_string(),
                destination: "conventions/Secret Plan.md".to_string(),
                destination_domain: None,
                permalink: Some("keep".to_string()),
                update_links: None,
            },
            None,
            &ada(),
        )
        .await
        .unwrap();
    let hers = engine
        .read_engram(&read_as("conventions/secret-plan"), &ada())
        .await
        .unwrap();
    assert_eq!(hers["path"], "conventions/Secret Plan.md", "{hers}");

    // Bob's grant covers one page: the refusal names the path he built from
    // his own title, never the name of a file only ada's draft holds.
    let (bob, join) = bob_in_adas_draft();
    let err = engine
        .write_engram_joined(
            &capture("Secret Plan", Some("conventions"), "x", true),
            None,
            &bob,
            Some(&join),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "this session is working inside ada's draft of 'conventions/code-review-standards', so a write to 'conventions/secret-plan.md' has nowhere to land: leave that draft first, and the write goes back to being your own"
    );
    assert!(!err.contains("Secret Plan.md"), "{err}");
}

#[tokio::test]
async fn an_overwrite_never_lands_on_a_manifest_with_a_custom_permalink() {
    let manifest = "---\ntype: manifest\ntitle: notes\npermalink: routing\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# notes\n\n## Scope\n\n- Everything about notes\n\n## When to Use\n\n- Route here for notes questions\n".to_string();
    let (tmp, engine, _scratch) = fixture(false, &[("MANIFEST.md", manifest.clone())]).await;
    let err = engine
        .write_engram(&capture("routing", None, "Not routing.", true))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "a new engram cannot be written at the domain root as MANIFEST.md: in this domain that file is the MANIFEST, which routing reads. Change it with edit_engram, or pick another title or a folder"
    );
    let notes = tmp.path().join("notes");
    assert_eq!(
        std::fs::read_to_string(notes.join("MANIFEST.md")).unwrap(),
        manifest
    );
    assert!(!notes.join("routing.md").exists());
}

#[tokio::test]
async fn a_write_the_index_refuses_leaves_the_file_as_it_found_it() {
    let old = engram("Code Review Standards", PERMALINK, "The old rule.");
    let (tmp, engine, _scratch) = fixture(false, &[(OWNER, old.clone())]).await;
    let notes = tmp.path().join("notes");
    // A transaction left open makes the index's own BEGIN fail, after the
    // file is already written: the one failure the pre-checks cannot see.
    let store = engine.store();
    store.lock().await.begin().await.unwrap();

    let replaced = engine
        .write_engram(&capture(
            "Code Review Standards",
            Some("conventions"),
            "The new rule.",
            true,
        ))
        .await;
    assert!(replaced.is_err(), "{replaced:?}");
    assert_eq!(
        std::fs::read_to_string(notes.join(OWNER)).unwrap(),
        old,
        "the old bytes are put back"
    );

    let created = engine
        .write_engram(&capture("Fresh Page", None, "New.", false))
        .await;
    assert!(created.is_err(), "{created:?}");
    assert!(
        !notes.join("fresh-page.md").exists(),
        "a new file is removed"
    );

    store.lock().await.rollback().await.unwrap();
}

/// Moves one of ada's drafts, keeping its permalink.
async fn ada_moves(engine: &Engine, identifier: &str, destination: &str) {
    engine
        .move_engram_as(
            &MoveParams {
                identifier: identifier.to_string(),
                domain: "notes".to_string(),
                destination: destination.to_string(),
                destination_domain: None,
                permalink: Some("keep".to_string()),
                update_links: None,
            },
            None,
            &ada(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_joined_landing_refusal_never_names_a_file_only_the_owners_draft_holds() {
    let (_tmp, engine, _scratch) = fixture(true, &[]).await;
    // The permalink the capture addresses lives in a draft-only file in
    // another folder...
    engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Private.",
                false,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap();
    ada_moves(&engine, PERMALINK, "private/Secret Plan.md").await;
    // ...and the slug path, which is the page granted to bob, holds another
    // engram of hers.
    engine
        .write_engram_as(
            &capture("Other Page", Some("conventions"), "Granted.", false),
            None,
            &ada(),
        )
        .await
        .unwrap();
    ada_moves(&engine, "conventions/other-page", SLUG).await;

    let (bob, mut join) = bob_in_adas_draft();
    join.path = SLUG.to_string();
    let err = engine
        .write_engram_joined(
            &capture("Code Review Standards", Some("conventions"), "x", true),
            None,
            &bob,
            Some(&join),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "this session is working inside ada's draft of 'conventions/other-page', so a write to 'conventions/code-review-standards.md' has nowhere to land: leave that draft first, and the write goes back to being your own"
    );
    assert!(
        !err.contains("Secret Plan") && !err.contains("private"),
        "{err}"
    );
}

// --- a plain create across folders (#112, amended 2026-09-28) ---------------

const CREATE_ACROSS: &str = "permalink 'conventions/code-review-standards' in domain 'notes' belongs to 'archive/code-review-standards.md' in folder 'archive', not in folder 'conventions'. A new engram cannot take the permalink of another engram: pick another title or folder, or change that engram in place with edit_engram";

#[tokio::test]
async fn a_create_whose_permalink_lives_in_another_folder_is_refused_without_the_offer() {
    let moved = "archive/code-review-standards.md";
    let old = engram("Code Review Standards", PERMALINK, "The old rule.");
    for review in [false, true] {
        let (tmp, engine, _scratch) = fixture(review, &[(moved, old.clone())]).await;
        let scope = if review { ada() } else { Scope::Unrestricted };
        let err = engine
            .write_engram_as(
                &capture(
                    "Code Review Standards",
                    Some("conventions"),
                    "The new rule.",
                    false,
                ),
                None,
                &scope,
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, CREATE_ACROSS, "review={review}");
        assert!(
            !err.contains("already exists in domain") && !err.contains("overwrite=true"),
            "no collision marker and no overwrite offer: {err}"
        );
        let notes = tmp.path().join("notes");
        assert!(!notes.join(SLUG).exists());
        assert_eq!(std::fs::read_to_string(notes.join(moved)).unwrap(), old);
    }
}

#[tokio::test]
async fn a_create_whose_permalink_lives_in_the_same_folder_keeps_the_collision() {
    let (_tmp, engine, _scratch) = fixture(
        false,
        &[(
            OWNER,
            engram("Code Review Standards", PERMALINK, "The old rule."),
        )],
    )
    .await;
    let err = engine
        .write_engram(&capture(
            "Code Review Standards",
            Some("conventions"),
            "The new rule.",
            false,
        ))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "permalink 'conventions/code-review-standards' already exists in domain 'notes' (at conventions/Code Review Standards.md); pass overwrite=true to replace"
    );
}

#[tokio::test]
async fn a_create_whose_title_namesake_lives_elsewhere_keeps_the_collision() {
    // `archive/g.md` is titled Gamma but answers to `g`: a namesake, not the
    // owner of `gamma`, so nothing says `gamma` belongs to it.
    let (_tmp, engine, _scratch) = fixture(
        false,
        &[("archive/g.md", engram("Gamma", "g", "A namesake."))],
    )
    .await;
    let err = engine
        .write_engram(&capture("Gamma", None, "The real gamma.", false))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "permalink 'gamma' already exists in domain 'notes' (at archive/g.md); pass overwrite=true to replace"
    );
}

const GAMMA_HELD: &str = "'gamma.md' in domain 'notes' already holds the engram 'custom-gamma'. Pick another title or folder, or change 'custom-gamma' with edit_engram";

#[tokio::test]
async fn a_create_at_a_slug_path_whose_namesake_answers_to_another_permalink_hears_m2() {
    // `gamma.md` is titled Gamma, so the title match finds it for `gamma`,
    // but it answers to `custom-gamma`: a create must not be offered an
    // overwrite that the landing then refuses.
    let other = engram("Gamma", "custom-gamma", "Not gamma.");
    let (tmp, engine, _scratch) = fixture(false, &[("gamma.md", other.clone())]).await;
    for overwrite in [false, true] {
        let err = engine
            .write_engram(&capture("Gamma", None, "Gamma body.", overwrite))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, GAMMA_HELD, "overwrite={overwrite}");
        assert!(
            !err.contains("already exists in domain") && !err.contains("overwrite=true"),
            "no collision marker and no overwrite offer: {err}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("notes/gamma.md")).unwrap(),
        other
    );
}

#[tokio::test]
async fn a_create_at_a_slug_path_the_writers_draft_holds_under_another_permalink_hears_m2() {
    let other = engram("Gamma", "custom-gamma", "Not gamma.");
    let (tmp, engine, _scratch) = fixture(true, &[("archive/else.md", other.clone())]).await;
    ada_moves(&engine, "custom-gamma", "gamma.md").await;
    for overwrite in [false, true] {
        let err = engine
            .write_engram_as(
                &capture("Gamma", None, "Gamma body.", overwrite),
                None,
                &ada(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, GAMMA_HELD, "overwrite={overwrite}");
        assert!(
            !err.contains("already exists in domain") && !err.contains("overwrite=true"),
            "no collision marker and no overwrite offer: {err}"
        );
    }
    let notes = tmp.path().join("notes");
    assert!(!notes.join("gamma.md").exists());
    assert_eq!(
        std::fs::read_to_string(notes.join("archive/else.md")).unwrap(),
        other
    );
}

// --- a create reads the permalink the way the writer's view holds it --------

#[tokio::test]
async fn a_create_whose_cross_folder_holder_the_writers_draft_deleted_lands() {
    let moved = "archive/code-review-standards.md";
    let old = engram("Code Review Standards", PERMALINK, "The old rule.");
    let (tmp, engine, _scratch) = fixture(true, &[(moved, old.clone())]).await;
    engine
        .delete_engram_as(
            &DeleteParams {
                identifier: PERMALINK.to_string(),
                domain: "notes".to_string(),
                expected_checksum: None,
            },
            None,
            &ada(),
        )
        .await
        .unwrap();
    // In ada's view nothing answers to the permalink any more, so her create
    // is a create: no M5 naming an engram she deleted, no collision either.
    let receipt = engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "The new rule.",
                false,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["path"], SLUG, "{receipt}");
    assert_eq!(receipt["draft"], true);
    assert_eq!(receipt["action"], "created");
    let notes = tmp.path().join("notes");
    assert_eq!(std::fs::read_to_string(notes.join(moved)).unwrap(), old);
}

#[tokio::test]
async fn a_create_whose_permalink_only_the_writers_own_draft_holds_elsewhere_hears_m5() {
    let (_tmp, engine, _scratch) = fixture(true, &[]).await;
    engine
        .write_engram_as(
            &capture("Code Review Standards", Some("conventions"), "Mine.", false),
            None,
            &ada(),
        )
        .await
        .unwrap();
    ada_moves(&engine, PERMALINK, "private/Secret Plan.md").await;
    let err = engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Again.",
                false,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "permalink 'conventions/code-review-standards' in domain 'notes' belongs to 'private/Secret Plan.md' in folder 'private', not in folder 'conventions'. A new engram cannot take the permalink of another engram: pick another title or folder, or change that engram in place with edit_engram"
    );
}

#[tokio::test]
async fn a_joined_create_across_folders_hears_m5_naming_only_the_team_file() {
    let moved = "archive/code-review-standards.md";
    let (_tmp, engine, _scratch) = fixture(
        true,
        &[(
            moved,
            engram("Code Review Standards", PERMALINK, "The team's rule."),
        )],
    )
    .await;
    let (bob, mut join) = bob_in_adas_draft();
    join.path = SLUG.to_string();
    let err = engine
        .write_engram_joined(
            &capture("Code Review Standards", Some("conventions"), "x", false),
            None,
            &bob,
            Some(&join),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, CREATE_ACROSS);
}

#[tokio::test]
async fn a_joined_create_never_hears_of_a_file_only_the_owners_draft_holds() {
    let (_tmp, engine, _scratch) = fixture(true, &[]).await;
    engine
        .write_engram_as(
            &capture(
                "Code Review Standards",
                Some("conventions"),
                "Private.",
                false,
            ),
            None,
            &ada(),
        )
        .await
        .unwrap();
    ada_moves(&engine, PERMALINK, "private/Secret Plan.md").await;
    // The page granted to bob sits at the slug path and is another engram.
    engine
        .write_engram_as(
            &capture("Other Page", Some("conventions"), "Granted.", false),
            None,
            &ada(),
        )
        .await
        .unwrap();
    ada_moves(&engine, "conventions/other-page", SLUG).await;
    let (bob, mut join) = bob_in_adas_draft();
    join.path = SLUG.to_string();
    let result = engine
        .write_engram_joined(
            &capture("Code Review Standards", Some("conventions"), "x", false),
            None,
            &bob,
            Some(&join),
        )
        .await;
    let text = match &result {
        Ok(receipt) => receipt.to_string(),
        Err(e) => e.to_string(),
    };
    assert!(
        !text.contains("Secret Plan") && !text.contains("private/"),
        "{text}"
    );
}
