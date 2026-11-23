//! The page settings (`freshness`, `follows`, `status`) and the page pact
//! files (`<page>.pact.md`) are a curator's: a page pact is written, deleted
//! and opened in a room by curators only, and moves and goes with its page;
//! a write below curate keeps the settings as stored.

use std::sync::Arc;

use crystalline_service::rest::{AuthStore, MemberLevel, ReflectionLevel, Role};
use crystalline_service::{DomainAccess, ReflectionCaller, Scope};

use crate::docs::{person, reflecting, write};
use crate::fixture::Fx;

const PAGE: &str = "# A\n\n## One\n\nFirst.\n";
const PACT: &str = "# Pact of A\n\n## Tone\n\nShort.\n";

/// `eng` with its Reflection on, private to `keeper`; `ann` a member who
/// edits the Reflection (contributor) and does not curate it. The machine
/// owner (`person()`) curates.
async fn editor_and_curator() -> (Fx, ReflectionCaller, ReflectionCaller) {
    let fx = reflecting().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "ann"] {
        auth.add_user(name, name, None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    auth.upsert_domain_member(
        "eng",
        "ann",
        MemberLevel::None,
        Some(ReflectionLevel::Contributor),
        "keeper",
    )
    .await
    .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let ann = ReflectionCaller::person(
        Scope::User {
            account: "ann".into(),
            admin: false,
        },
        "ann",
    );
    (fx, ann, person())
}

fn file(fx: &Fx, rel: &str) -> Option<String> {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).ok()
}

async fn put(
    fx: &Fx,
    path: &str,
    text: &str,
    who: &ReflectionCaller,
) -> Result<serde_json::Value, String> {
    fx.engine
        .write_reflection(&write(path, text), who)
        .await
        .map_err(|e| e.to_string())
}

/// A page pact is written by curators and read by everyone who reads the
/// page; its room takes curate too.
#[tokio::test]
async fn a_page_pact_is_written_only_by_a_curator() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &ann).await.unwrap();
    let err = put(&fx, "guides/a.pact.md", PACT, &ann).await.unwrap_err();
    assert!(err.contains("curate is needed"), "{err}");
    assert!(file(&fx, "guides/a.pact.md").is_none());
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    let err = put(&fx, "guides/a.pact.md", "# Mine\n", &ann)
        .await
        .unwrap_err();
    assert!(err.contains("curate is needed"), "{err}");
    let read = fx
        .engine
        .read_reflection("eng", "guides/a.pact.md", &ann)
        .await
        .unwrap();
    assert!(read.to_string().contains("Short."), "{read}");
    assert!(
        fx.engine
            .require_reflection_edit("eng", "guides/a.pact.md", &ann)
            .await
            .is_err(),
        "the pact's room takes curate"
    );
    fx.engine
        .require_reflection_edit("eng", "guides/a.md", &ann)
        .await
        .unwrap();
    fx.engine
        .require_reflection_edit("eng", "guides/a.pact.md", &curator)
        .await
        .unwrap();
}

/// A page pact alone is deleted by a curator only; its page stays.
#[tokio::test]
async fn a_page_pact_alone_is_deleted_only_by_a_curator() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    let err = fx
        .engine
        .delete_reflection("eng", "guides/a.pact.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("curate is needed"), "{err}");
    assert!(file(&fx, "guides/a.pact.md").is_some());
    fx.engine
        .delete_reflection("eng", "guides/a.pact.md", &curator)
        .await
        .unwrap();
    assert!(file(&fx, "guides/a.pact.md").is_none());
    assert!(file(&fx, "guides/a.md").is_some());
    // PACT.md is nobody's to delete, and an editor hears why.
    let err = fx
        .engine
        .delete_reflection("eng", "PACT.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("agreement"), "{err}");
}

/// A page pact moves with its page and never on its own, and no page moves
/// onto a pact file's path.
#[tokio::test]
async fn a_page_pact_is_never_moved_on_its_own() {
    let (fx, _ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    for (from, to) in [
        ("guides/a.pact.md", "guides/c.pact.md"),
        ("guides/b.md", "guides/c.pact.md"),
    ] {
        let err = fx
            .engine
            .move_reflection("eng", from, to, &curator)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("moves with its page"), "{from} -> {to}: {err}");
    }
    assert!(file(&fx, "guides/a.pact.md").is_some());
    assert!(file(&fx, "guides/b.md").is_some());
}

const SET: &str = "---\ntitle: A\nfreshness: 1d\nfollows: guides/b.md\nstatus: current\n---\n\n# A\n\n## One\n\nFirst.\n";

/// A room never shows the settings, and its saves never change them: a save
/// of the room text as it came is no write at all, and a key typed into the
/// editor is replaced by the stored one.
#[tokio::test]
async fn a_room_neither_shows_nor_changes_the_settings() {
    let (fx, _ann, curator) = editor_and_curator().await;
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.md", SET, &curator).await.unwrap();
    let stored = file(&fx, "guides/a.md").unwrap();
    let room = fx
        .engine
        .reflection_room_text("eng", "guides/a.md")
        .await
        .unwrap();
    for key in ["freshness", "follows", "status", "reflection:"] {
        assert!(!room.content.contains(key), "{key}: {}", room.content);
    }
    assert!(room.content.contains("title: A"), "{}", room.content);
    let saved = fx
        .engine
        .save_reflection_room(
            "eng",
            "guides/a.md",
            &room.content,
            &room.checksum,
            &crystalline_service::SectionAuthors::plain(vec!["ann".into()]),
            true,
        )
        .await
        .unwrap();
    assert_eq!(file(&fx, "guides/a.md").unwrap(), stored, "no change");
    assert_eq!(saved["checksum"], room.checksum.as_str());

    let typed = room
        .content
        .replace("title: A\n", "title: A\nfreshness: 1h\nstatus: archived\n")
        .replace("First.", "First, changed.");
    fx.engine
        .save_reflection_room(
            "eng",
            "guides/a.md",
            &typed,
            &room.checksum,
            &crystalline_service::SectionAuthors::plain(vec!["ann".into()]),
            false,
        )
        .await
        .unwrap();
    let after = file(&fx, "guides/a.md").unwrap();
    assert!(after.contains("First, changed."), "{after}");
    assert!(
        after.starts_with("---\ntitle: A\nfreshness: 1d\nfollows: guides/b.md\nstatus: current\n"),
        "{after}"
    );
}

async fn put_err(fx: &Fx, path: &str, text: &str, who: &ReflectionCaller) -> String {
    put(fx, path, text, who).await.unwrap_err()
}

fn settings(fx: &Fx, path: &str) -> String {
    let text = file(fx, path).unwrap();
    let keys = crystalline_core::reflection::page_key_values(&text);
    keys.iter()
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("?")))
        .collect::<Vec<_>>()
        .join(",")
}

/// A whole-page write below curate keeps the settings as stored: a page
/// sent without them keeps them, one that sends them as stored passes, and
/// one that sends other values is refused. `status` is the curator's only
/// for the retired words.
#[tokio::test]
async fn an_editors_write_keeps_the_settings_and_is_refused_when_it_changes_them() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.md", SET, &curator).await.unwrap();
    let all = "follows=guides/b.md,freshness=1d,status=current";
    assert_eq!(settings(&fx, "guides/a.md"), all);

    put(&fx, "guides/a.md", "# A\n\n## One\n\nChanged.\n", &ann)
        .await
        .unwrap();
    assert_eq!(settings(&fx, "guides/a.md"), all, "kept when not sent");
    assert!(file(&fx, "guides/a.md").unwrap().contains("Changed."));
    put(&fx, "guides/a.md", SET, &ann).await.unwrap();
    assert_eq!(settings(&fx, "guides/a.md"), all, "sent as stored");

    for (key, changed) in [
        ("freshness", SET.replace("freshness: 1d", "freshness: 6h")),
        (
            "follows",
            SET.replace("follows: guides/b.md", "follows: guides/c.md"),
        ),
        ("status", SET.replace("status: current", "status: archived")),
    ] {
        let err = put_err(&fx, "guides/a.md", &changed, &ann).await;
        assert!(err.contains("curate"), "{key}: {err}");
        assert!(err.contains(key), "{key}: {err}");
    }
    assert_eq!(settings(&fx, "guides/a.md"), all);

    // A status that is no retired word is anybody's to change.
    put(
        &fx,
        "guides/a.md",
        &SET.replace("status: current", "status: draft"),
        &ann,
    )
    .await
    .unwrap();
    assert!(settings(&fx, "guides/a.md").ends_with("status=draft"));
    // A retired page stays retired, whatever an editor sends.
    put(
        &fx,
        "guides/a.md",
        &SET.replace("status: current", "status: Archived"),
        &curator,
    )
    .await
    .unwrap();
    let err = put_err(&fx, "guides/a.md", SET, &ann).await;
    assert!(err.contains("status"), "{err}");
    put(&fx, "guides/a.md", PAGE, &ann).await.unwrap();
    assert!(settings(&fx, "guides/a.md").ends_with("status=Archived"));

    // A section edit leaves the frontmatter alone.
    fx.engine
        .write_reflection(
            &crystalline_service::params::WriteReflectionParams {
                domain: "eng".into(),
                path: "guides/a.md".into(),
                operation: Some("replace_section".into()),
                section: Some("One".into()),
                content: Some("By section.".into()),
                ..Default::default()
            },
            &ann,
        )
        .await
        .unwrap();
    assert!(settings(&fx, "guides/a.md").starts_with("follows=guides/b.md,freshness=1d"));
}

/// A new page an editor writes carries no settings a curator sets.
#[tokio::test]
async fn an_editor_creates_a_page_without_the_curators_settings() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    for (key, text) in [
        ("freshness", "---\nfreshness: 1d\n---\n\n# N\n"),
        ("follows", "---\nfollows: guides/b.md\n---\n\n# N\n"),
        ("status", "---\nstatus: archived\n---\n\n# N\n"),
    ] {
        let err = put_err(&fx, "guides/n.md", text, &ann).await;
        assert!(err.contains("curate") && err.contains(key), "{key}: {err}");
        assert!(file(&fx, "guides/n.md").is_none());
    }
    put(&fx, "guides/n.md", "---\nstatus: draft\n---\n\n# N\n", &ann)
        .await
        .unwrap();
    assert_eq!(settings(&fx, "guides/n.md"), "status=draft");
}

/// A curator sets and clears the settings by writing the page; a page sent
/// without them keeps them, and a key with no value clears it.
#[tokio::test]
async fn a_curator_sets_keeps_and_clears_the_settings() {
    let (fx, _ann, curator) = editor_and_curator().await;
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.md", SET, &curator).await.unwrap();
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    assert_eq!(
        settings(&fx, "guides/a.md"),
        "follows=guides/b.md,freshness=1d,status=current"
    );
    put(
        &fx,
        "guides/a.md",
        &SET.replace("freshness: 1d", "freshness: 6h"),
        &curator,
    )
    .await
    .unwrap();
    assert_eq!(
        settings(&fx, "guides/a.md"),
        "follows=guides/b.md,freshness=6h,status=current"
    );
    put(
        &fx,
        "guides/a.md",
        "---\nfreshness:\nfollows:\n---\n\n# A\n",
        &curator,
    )
    .await
    .unwrap();
    assert_eq!(settings(&fx, "guides/a.md"), "status=current");
}

/// A follows is written only when it names an existing page of the same
/// Reflection and closes no cycle; the refusal names the cycle.
#[tokio::test]
async fn a_follows_that_names_no_page_or_closes_a_cycle_is_refused() {
    let (fx, _ann, curator) = editor_and_curator().await;
    put(
        &fx,
        "guides/b.md",
        "---\nfollows: guides/c.md\n---\n\n# B\n",
        &curator,
    )
    .await
    .unwrap_err();
    put(&fx, "guides/c.md", PAGE, &curator).await.unwrap();
    put(
        &fx,
        "guides/b.md",
        "---\nfollows: guides/c.md\n---\n\n# B\n",
        &curator,
    )
    .await
    .unwrap();
    put(
        &fx,
        "guides/a.md",
        "---\nfollows: guides/b.md\n---\n\n# A\n",
        &curator,
    )
    .await
    .unwrap();
    let err = put_err(
        &fx,
        "guides/c.md",
        "---\nfollows: guides/a.md\n---\n\n# C\n",
        &curator,
    )
    .await;
    assert!(
        err.contains("guides/c.md > guides/a.md > guides/b.md > guides/c.md"),
        "{err}"
    );
    assert!(!file(&fx, "guides/c.md").unwrap().contains("follows"));
    for (target, says) in [
        ("guides/gone.md", "does not exist"),
        ("crystalline://ops/x.md", "another domain"),
        ("PACT.md", "names no page"),
    ] {
        let err = put_err(
            &fx,
            "guides/c.md",
            &format!("---\nfollows: {target}\n---\n\n# C\n"),
            &curator,
        )
        .await;
        assert!(err.contains(says), "{target}: {err}");
    }
}

fn exists(fx: &Fx, rel: &str) -> bool {
    fx.root.join("reflections").join(rel).exists()
}

/// A page with a page pact moves only with a curator, and its pact moves
/// with it: the pact, its baseline and its threads, and the index rows.
#[tokio::test]
async fn a_page_moves_with_its_pact_and_only_by_a_curator() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    std::fs::write(
        fx.root.join("reflections/guides/a.pact.threads.md"),
        "# Threads\n",
    )
    .unwrap();
    std::fs::create_dir_all(fx.root.join("reflections/.reflected/guides")).unwrap();
    std::fs::write(fx.root.join("reflections/.reflected/guides/a.pact.md"), "").unwrap();
    let err = fx
        .engine
        .move_reflection("eng", "guides/a.md", "docs/a.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("curate") && err.contains("guides/a.pact.md"),
        "{err}"
    );
    assert!(exists(&fx, "guides/a.md") && exists(&fx, "guides/a.pact.md"));

    let moved = fx
        .engine
        .move_reflection("eng", "guides/a.md", "docs/a.md", &curator)
        .await
        .unwrap();
    assert_eq!(moved["pact_moved"], true, "{moved}");
    for gone in [
        "guides/a.md",
        "guides/a.pact.md",
        "guides/a.pact.threads.md",
        ".reflected/guides/a.pact.md",
    ] {
        assert!(!exists(&fx, gone), "{gone}");
    }
    for there in [
        "docs/a.md",
        "docs/a.pact.md",
        "docs/a.pact.threads.md",
        ".reflected/docs/a.pact.md",
    ] {
        assert!(exists(&fx, there), "{there}");
    }
    let read = fx
        .engine
        .read_reflection("eng", "docs/a.pact.md", &ann)
        .await
        .unwrap();
    assert!(read.to_string().contains("Short."), "{read}");
    assert!(
        fx.engine
            .read_reflection("eng", "guides/a.pact.md", &ann)
            .await
            .is_err()
    );
}

/// A move onto a path that becomes the folder parent of pages already
/// there changes the pact those pages follow: a curator's move.
#[tokio::test]
async fn a_move_onto_a_folder_parent_needs_curate() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/x/one.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    let err = fx
        .engine
        .move_reflection("eng", "guides/b.md", "guides/x.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("curate") && err.contains("folder parent"),
        "{err}"
    );
    fx.engine
        .move_reflection("eng", "guides/b.md", "guides/y.md", &ann)
        .await
        .unwrap();
    fx.engine
        .move_reflection("eng", "guides/y.md", "guides/x.md", &curator)
        .await
        .unwrap();
    assert!(exists(&fx, "guides/x.md"));
}

/// The pages that follow a moved page follow it to its new path, in the
/// same move.
#[tokio::test]
async fn a_follows_names_the_new_path_of_a_moved_page() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    put(
        &fx,
        "notes/c.md",
        "---\ntitle: C\nfollows: guides/b.md\n---\n\n# C\n",
        &curator,
    )
    .await
    .unwrap();
    let moved = fx
        .engine
        .move_reflection("eng", "guides/b.md", "guides/d.md", &ann)
        .await
        .unwrap();
    assert_eq!(moved["follows_rewritten"], 1, "{moved}");
    assert_eq!(settings(&fx, "notes/c.md"), "follows=guides/d.md");
    // The index row was rewritten in the same move.
    let read = fx
        .engine
        .read_reflection("eng", "notes/c.md", &curator)
        .await
        .unwrap();
    assert!(read.to_string().contains("follows: guides/d.md"), "{read}");
}

/// A loose page pact at the target is no file a move adopts.
#[tokio::test]
async fn a_move_does_not_adopt_a_loose_page_pact() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/z.pact.md", PACT, &curator).await.unwrap();
    let err = fx
        .engine
        .move_reflection("eng", "guides/b.md", "guides/z.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("guides/z.pact.md"), "{err}");
    assert!(exists(&fx, "guides/b.md"));
    // Nor does an editor's new page there take it; a curator's does.
    let err = put_err(&fx, "guides/z.md", PAGE, &ann).await;
    assert!(
        err.contains("guides/z.pact.md") && err.contains("curate"),
        "{err}"
    );
    assert!(!exists(&fx, "guides/z.md"));
    put(&fx, "guides/z.md", PAGE, &curator).await.unwrap();
}

/// A page with a page pact is deleted by a curator only, and its pact goes
/// with it: the pact, its baseline and its threads.
#[tokio::test]
async fn a_page_is_deleted_with_its_pact_and_only_by_a_curator() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    std::fs::write(
        fx.root.join("reflections/guides/a.pact.threads.md"),
        "# Threads\n",
    )
    .unwrap();
    std::fs::create_dir_all(fx.root.join("reflections/.reflected/guides")).unwrap();
    std::fs::write(fx.root.join("reflections/.reflected/guides/a.pact.md"), "").unwrap();
    let err = fx
        .engine
        .delete_reflection("eng", "guides/a.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("curate") && err.contains("guides/a.pact.md"),
        "{err}"
    );
    assert!(exists(&fx, "guides/a.md"));
    fx.engine
        .delete_reflection("eng", "guides/a.md", &curator)
        .await
        .unwrap();
    for gone in [
        "guides/a.md",
        "guides/a.pact.md",
        "guides/a.pact.threads.md",
        ".reflected/guides/a.pact.md",
    ] {
        assert!(!exists(&fx, gone), "{gone}");
    }
    assert!(
        fx.engine
            .read_reflection("eng", "guides/a.pact.md", &curator)
            .await
            .is_err()
    );
}

/// A page pact spelled in another letter case is the same file on a
/// case-folding disk: an editor neither writes, opens nor deletes it under
/// that spelling.
#[tokio::test]
async fn a_page_pact_in_another_letter_case_is_still_the_curators() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    let pact = file(&fx, "guides/a.pact.md");
    for spelled in ["guides/a.Pact.md", "guides/a.PACT.md", "guides/A.PACT.MD"] {
        let err = put_err(&fx, spelled, "# Mine\n", &ann).await;
        assert!(
            err.contains("curate is needed") || err.contains("spell it"),
            "{spelled}: {err}"
        );
        assert!(
            fx.engine
                .require_reflection_edit("eng", spelled, &ann)
                .await
                .is_err(),
            "{spelled}: the room"
        );
        let err = fx
            .engine
            .delete_reflection("eng", spelled, &ann)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("curate is needed") || err.contains("spell it"),
            "{spelled}: {err}"
        );
    }
    assert_eq!(file(&fx, "guides/a.pact.md"), pact, "untouched");
}

/// A frontmatter that is no valid mapping as a whole (a key written twice)
/// does not hide the page keys from the gate, and a page key written twice
/// is refused.
#[tokio::test]
async fn a_duplicate_key_does_not_hide_the_settings_from_the_gate() {
    let (fx, ann, curator) = editor_and_curator().await;
    let planted = "---\ntitle: A\ntitle: B\nfreshness: 1h\n---\n\n# A\n";
    let err = put_err(&fx, "guides/n.md", planted, &ann).await;
    assert!(err.contains("freshness") && err.contains("curate"), "{err}");
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    let err = put_err(&fx, "guides/a.md", planted, &ann).await;
    assert!(err.contains("freshness") && err.contains("curate"), "{err}");
    let twice = "---\nstatus: current\nstatus: archived\n---\n\n# A\n";
    for who in [&ann, &curator] {
        let err = put_err(&fx, "guides/a.md", twice, who).await;
        assert!(err.contains("more than once"), "{err}");
    }
}
