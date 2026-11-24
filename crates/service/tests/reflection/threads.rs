//! Threads through the engine.

use crystalline_service::ReflectionCaller;
use crystalline_service::SectionAuthors;
use crystalline_service::params::DiscussParams;

use crate::docs::{agent, person, write};
use crate::fixture::manifest;
use crate::rights::viewer_engine;

const PAGE: &str = "# D\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night.\n\n## Typed by hand\n\nNo id yet.\n";

fn open(section: &str, quote: Option<&str>, text: &str) -> DiscussParams {
    DiscussParams {
        domain: "eng".into(),
        action: Some("open".into()),
        path: Some("d.md".into()),
        section: Some(section.into()),
        quote: quote.map(str::to_string),
        text: Some(text.into()),
        ..Default::default()
    }
}

fn step(action: &str, id: &str, text: &str) -> DiscussParams {
    DiscussParams {
        domain: "eng".into(),
        action: Some(action.into()),
        path: Some("d.md".into()),
        thread: Some(id.into()),
        text: Some(text.into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_viewer_opens_answers_resolves_and_reopens() {
    let (fx, vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let viewer = ReflectionCaller::person(vera, "vera");
    let opened = fx
        .engine
        .discuss(
            &open("aaaa1111", Some("run at night"), "Still true?"),
            &viewer,
        )
        .await
        .unwrap();
    let id = opened["id"].as_str().unwrap().to_string();
    assert_eq!(opened["waiting_on"], "agent");
    assert_eq!(opened["anchor"]["state"], "found");
    let answered = fx
        .engine
        .discuss(&step("answer", &id, "Yes, still true."), &agent())
        .await
        .unwrap();
    assert_eq!(answered["waiting_on"], "people");
    let resolved = fx
        .engine
        .discuss(&step("resolve", &id, "  "), &viewer)
        .await
        .expect("a resolve needs no outcome");
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(resolved["entries"][2]["kind"], "resolve");
    assert_eq!(
        resolved["entries"][2]["text"], "",
        "a resolve without an outcome has no text"
    );
    let reopened = fx
        .engine
        .discuss(&step("reopen", &id, "It changed again."), &viewer)
        .await
        .unwrap();
    assert_eq!(reopened["status"], "open");
    assert_eq!(
        reopened["entries"].as_array().unwrap().len(),
        4,
        "the reopen is a visible entry"
    );
    let listed = fx
        .engine
        .list_reflection_threads("eng", None, "all", &viewer)
        .await
        .unwrap();
    assert_eq!(listed["threads"].as_array().unwrap().len(), 1);
}

/// A person's thread stays open until the person resolves it: an agent that
/// answers and resolves in one call would hide its answer in a closed thread.
#[tokio::test]
async fn an_agent_answers_a_persons_thread_and_leaves_it_open() {
    let (fx, vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let viewer = ReflectionCaller::person(vera, "vera");
    let opened = fx
        .engine
        .discuss(&open("aaaa1111", None, "Still true?"), &viewer)
        .await
        .unwrap();
    let id = opened["id"].as_str().unwrap().to_string();
    let refused = |e: crystalline_service::EngineError| match e {
        crystalline_service::EngineError::Invalid(why) => {
            assert!(
                why.contains("leave it open")
                    && why.contains("the person who opened it resolves it"),
                "{why}"
            );
        }
        other => panic!("refused as invalid: {other:?}"),
    };
    refused(
        fx.engine
            .discuss(&step("resolve", &id, "Yes, still true."), &agent())
            .await
            .unwrap_err(),
    );
    fx.engine
        .discuss(&step("answer", &id, "Yes, still true."), &agent())
        .await
        .unwrap();
    refused(
        fx.engine
            .discuss(&step("resolve", &id, "Answered."), &agent())
            .await
            .unwrap_err(),
    );
    fx.engine
        .discuss(&step("answer", &id, "Thanks, you can close it."), &viewer)
        .await
        .unwrap();
    let resolved = fx
        .engine
        .discuss(&step("resolve", &id, "Confirmed by vera."), &agent())
        .await
        .unwrap();
    assert_eq!(resolved["status"], "resolved");
}

/// The anonymous visitor of a public domain reads the threads and writes none.
#[tokio::test]
async fn an_anonymous_visitor_reads_threads_and_cannot_comment() {
    let (fx, vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(
            &open("aaaa1111", None, "Still true?"),
            &ReflectionCaller::person(vera, "vera"),
        )
        .await
        .unwrap();
    let id = opened["id"].as_str().unwrap().to_string();
    let anonymous = ReflectionCaller::person(crystalline_service::Scope::Anonymous, "anonymous");
    let listed = fx
        .engine
        .list_reflection_threads("eng", None, "open", &anonymous)
        .await
        .unwrap();
    assert_eq!(listed["threads"].as_array().unwrap().len(), 1, "it reads");
    for p in [
        open("aaaa1111", None, "Hello?"),
        step("answer", &id, "Me too."),
    ] {
        let err = fx
            .engine
            .discuss(&p, &anonymous)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("sign in to comment"), "{err}");
    }
}

/// A reader opens a thread on a heading a plain editor added: the page is
/// not rewritten (its bytes and checksum stay), the thread anchors on the
/// heading and moves onto the id a later write gives the section.
#[tokio::test]
async fn a_thread_on_a_heading_without_an_id_leaves_the_page_alone() {
    let (fx, vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/d.md");
    let text = std::fs::read_to_string(&path).unwrap() + "\n## Added later\n\nBy hand.\n";
    std::fs::write(&path, &text).unwrap();
    let viewer = ReflectionCaller::person(vera, "vera");
    let checksum = |fx: &crate::fixture::Fx, who: &ReflectionCaller| {
        let fx = fx.engine.clone();
        let who = who.clone();
        async move {
            fx.read_reflection("eng", "d.md", &who).await.unwrap()["checksum"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let before = checksum(&fx, &viewer).await;
    let opened = fx
        .engine
        .discuss(
            &open("Added later", Some("By hand"), "Who added this?"),
            &viewer,
        )
        .await
        .unwrap();
    assert_eq!(opened["anchor"]["section"], "Added later");
    assert_eq!(opened["anchor"]["state"], "found");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "the page is not rewritten"
    );
    assert_eq!(checksum(&fx, &viewer).await, before);
    let threads = std::fs::read_to_string(fx.root.join("reflections/d.threads.md")).unwrap();
    assert!(threads.contains("heading: Added later\n"), "{threads}");

    // Anchoring on an id-less heading answers no feedback item: refused,
    // nothing written.
    let mut answering = open("Added later", None, "I read your change.");
    answering.answers_feedback = true;
    let err = fx.engine.discuss(&answering, &agent()).await.unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Invalid(_)),
        "{err:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

    // A later write gives the section its id; the thread still lists under it.
    fx.engine
        .write_reflection(&write("d.md", &text), &person())
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("## Added later\n<!-- r:")
    );
    let listed = fx
        .engine
        .list_reflection_threads("eng", Some("d.md"), "open", &viewer)
        .await
        .unwrap();
    let thread = &listed["threads"][0];
    assert_eq!(thread["id"], opened["id"]);
    assert_ne!(thread["anchor"]["section"], "Added later", "{thread}");
    assert_eq!(thread["anchor"]["state"], "found");
}

/// The id a write gives a heading is the one its threads anchor on from
/// then: the write rewrites their `heading:` anchors into ids under the page
/// lock, so a later rename of the heading keeps each thread on its section.
/// A write that renames an id-less heading in the same go leaves its thread
/// anchored by the old heading, exactly as it was: it is never lost.
#[tokio::test]
async fn a_write_that_gives_a_heading_its_id_moves_its_threads_onto_it() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/d.md");
    let threads_path = fx.root.join("reflections/d.threads.md");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n## Kept\n\nKept text.\n\n## Renamed\n\nRenamed text.\n";
    std::fs::write(&path, &text).unwrap();
    let kept = fx
        .engine
        .discuss(&open("Kept", Some("Kept text"), "Why?"), &person())
        .await
        .unwrap();
    let renamed = fx
        .engine
        .discuss(&open("Renamed", None, "And this?"), &person())
        .await
        .unwrap();
    // One write gives both headings their ids, and renames the second.
    let receipt = fx
        .engine
        .write_reflection(
            &write("d.md", &text.replace("## Renamed", "## Renamed now")),
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["ids_assigned"].as_array().unwrap().len(), 2);
    let page = std::fs::read_to_string(&path).unwrap();
    let id = page
        .split("## Kept\n<!-- r:")
        .nth(1)
        .and_then(|rest| rest.split(" -->").next())
        .expect("Kept has its id")
        .to_string();
    let threads = std::fs::read_to_string(&threads_path).unwrap();
    assert!(threads.contains(&format!("section: {id}\n")), "{threads}");
    assert!(threads.contains("heading: Renamed\n"), "{threads}");

    // A rename of the heading now keeps the thread on its section.
    let page = page.replace("## Kept\n", "## Kept for good\n");
    fx.engine
        .write_reflection(&write("d.md", &page), &person())
        .await
        .unwrap();
    let listed = fx
        .engine
        .list_reflection_threads("eng", Some("d.md"), "open", &person())
        .await
        .unwrap();
    let by_id = |want: &serde_json::Value| {
        listed["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == *want)
            .cloned()
            .expect("the thread is listed")
    };
    let thread = by_id(&kept["id"]);
    assert_eq!(thread["anchor"]["section"], id.as_str());
    assert_eq!(thread["anchor"]["state"], "found", "{thread}");
    let thread = by_id(&renamed["id"]);
    assert_eq!(thread["anchor"]["section"], "Renamed");
    assert_eq!(thread["anchor"]["state"], "section_gone", "{thread}");
}

/// A room's closing save gives a heading typed in the room its id, and the
/// thread opened on that heading moves onto it, like a write.
#[tokio::test]
async fn a_rooms_closing_save_moves_a_heading_thread_onto_the_new_id() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/d.md");
    let stored = std::fs::read_to_string(&path).unwrap();
    let typed = stored.clone() + "\n## Typed in the room\n\nRoom text.\n";
    let saved = fx
        .engine
        .save_reflection_room(
            "eng",
            "d.md",
            &typed,
            &crate::support::sha256_hex(stored.as_bytes()),
            &SectionAuthors::plain(vec!["jordi".to_string()]),
            false,
        )
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(&open("Typed in the room", None, "New?"), &person())
        .await
        .unwrap();
    assert_eq!(opened["anchor"]["section"], "Typed in the room");
    let current = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        saved["checksum"],
        crate::support::sha256_hex(current.as_bytes())
    );
    fx.engine
        .save_reflection_room(
            "eng",
            "d.md",
            &current,
            &crate::support::sha256_hex(current.as_bytes()),
            &SectionAuthors::default(),
            true,
        )
        .await
        .unwrap();
    let page = std::fs::read_to_string(&path).unwrap();
    assert!(page.contains("## Typed in the room\n<!-- r:"), "{page}");
    let threads = std::fs::read_to_string(fx.root.join("reflections/d.threads.md")).unwrap();
    assert!(!threads.contains("heading:"), "{threads}");
    let renamed = page.replace("## Typed in the room\n", "## Typed and renamed\n");
    fx.engine
        .write_reflection(&write("d.md", &renamed), &person())
        .await
        .unwrap();
    let listed = fx
        .engine
        .list_reflection_threads("eng", Some("d.md"), "open", &person())
        .await
        .unwrap();
    assert_eq!(listed["threads"][0]["id"], opened["id"]);
    assert_eq!(listed["threads"][0]["anchor"]["state"], "found", "{listed}");
}

/// An empty quote discusses the whole section and is not stored.
#[tokio::test]
async fn an_empty_quote_is_left_out() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(&open("aaaa1111", Some("  "), "Still true?"), &person())
        .await
        .unwrap();
    assert!(opened["anchor"]["quote"].is_null(), "{opened}");
    let threads = std::fs::read_to_string(fx.root.join("reflections/d.threads.md")).unwrap();
    assert!(!threads.contains("quote:"), "{threads}");
}

/// A page or a section that is not there: the page is not found, the
/// section is a malformed request; an answer on a thread whose page is gone
/// is not found either.
#[tokio::test]
async fn a_missing_page_or_section_is_refused() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let mut elsewhere = open("aaaa1111", None, "Hello?");
    elsewhere.path = Some("nowhere.md".into());
    let err = fx.engine.discuss(&elsewhere, &person()).await.unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::NotFound(_)),
        "{err:?}"
    );
    let err = fx
        .engine
        .discuss(&open("No such section", None, "Hello?"), &person())
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Invalid(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("No such section"), "{err}");
    assert!(!fx.root.join("reflections/d.threads.md").exists());

    let opened = fx
        .engine
        .discuss(&open("aaaa1111", None, "Still true?"), &person())
        .await
        .unwrap();
    // The page goes outside Crystalline; its threads file stays.
    std::fs::remove_file(fx.root.join("reflections/d.md")).unwrap();
    let threads = fx.root.join("reflections/d.threads.md");
    let before = std::fs::read_to_string(&threads).unwrap();
    let err = fx
        .engine
        .discuss(
            &step("answer", opened["id"].as_str().unwrap(), "Yes."),
            &agent(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::NotFound(_)),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "no reflection 'd.md' in domain 'eng'");
    assert_eq!(std::fs::read_to_string(&threads).unwrap(), before);
}

/// The anchor follows the page: an unrelated edit keeps it found, a move
/// carries the thread to the new path, and a rewrite of the quoted text
/// answers text_changed.
#[tokio::test]
async fn an_anchor_follows_an_edit_and_a_move() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(
            &open("aaaa1111", Some("run at night"), "Still true?"),
            &person(),
        )
        .await
        .unwrap();
    let state = |listed: &serde_json::Value| listed["threads"][0]["anchor"]["state"].clone();
    let edited = PAGE.replace("No id yet.", "Still no news.");
    fx.engine
        .write_reflection(&write("d.md", &edited), &person())
        .await
        .unwrap();
    let listed = fx
        .engine
        .list_reflection_threads("eng", Some("d.md"), "open", &person())
        .await
        .unwrap();
    assert_eq!(state(&listed), "found", "{listed}");

    fx.engine
        .move_reflection("eng", "d.md", "e.md", &person())
        .await
        .unwrap();
    let old = fx
        .engine
        .list_reflection_threads("eng", Some("d.md"), "all", &person())
        .await
        .unwrap();
    assert!(old["threads"].as_array().unwrap().is_empty(), "{old}");
    let moved = fx
        .engine
        .list_reflection_threads("eng", Some("e.md"), "open", &person())
        .await
        .unwrap();
    assert_eq!(moved["threads"][0]["id"], opened["id"]);
    assert_eq!(moved["threads"][0]["path"], "e.md");
    assert_eq!(state(&moved), "found", "{moved}");

    let rewritten = edited.replace("Deploys run at night.", "Deploys run in the morning.");
    fx.engine
        .write_reflection(&write("e.md", &rewritten), &person())
        .await
        .unwrap();
    let listed = fx
        .engine
        .list_reflection_threads("eng", Some("e.md"), "open", &person())
        .await
        .unwrap();
    assert_eq!(state(&listed), "text_changed", "{listed}");
}

#[tokio::test]
async fn an_agent_speaks_to_people_without_engram_links() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .discuss(
            &open("aaaa1111", None, "See [[runbooks/deploy]] for why."),
            &agent(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("plain words"), "{err}");
    let err = fx
        .engine
        .discuss(
            &open("aaaa1111", None, "See crystalline://eng/runbooks/deploy"),
            &agent(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("plain words"), "{err}");
    fx.engine
        .discuss(
            &open("aaaa1111", None, "A person may write [[this]]."),
            &person(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_quote_that_is_not_there_is_refused() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .discuss(&open("aaaa1111", Some("in the morning"), "?"), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("in the morning"), "{err}");
}

/// An unknown thread is not found (404 over REST), not a
/// malformed request.
#[tokio::test]
async fn an_unknown_thread_is_not_found() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    fx.engine
        .discuss(&open("aaaa1111", None, "Still true?"), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .discuss(&step("answer", "t-0000dead", "Yes."), &agent())
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::NotFound(_)),
        "{err:?}"
    );
}

/// An open that is refused (a quote that is not there)
/// writes nothing, not even the id a heading would have got.
#[tokio::test]
async fn a_refused_open_writes_no_id() {
    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/d.md");
    let text = std::fs::read_to_string(&path).unwrap() + "\n## Added later\n\nBy hand.\n";
    std::fs::write(&path, &text).unwrap();
    let err = fx
        .engine
        .discuss(
            &open("Added later", Some("not in the section"), "?"),
            &person(),
        )
        .await;
    assert!(err.is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        text,
        "no id was written for a refused open"
    );
    assert!(!fx.root.join("reflections/d.threads.md").exists());
}

#[tokio::test]
async fn answering_a_feedback_item_needs_curate_and_marks_the_section() {
    let (fx, vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let mut p = open(
        "aaaa1111",
        None,
        "I read your change; I think the batch moved.",
    );
    p.answers_feedback = true;
    let err = fx
        .engine
        .discuss(&p, &ReflectionCaller::person(vera, "vera"))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("curate"), "{err}");
    let opened = fx.engine.discuss(&p, &agent()).await.unwrap();
    let text = std::fs::read_to_string(fx.root.join("reflections/d.md")).unwrap();
    let prov = crystalline_core::reflection::read_provenance(&text);
    assert_eq!(
        prov["aaaa1111"].answered_by.as_deref(),
        opened["id"].as_str()
    );
}

/// 0.25.0's read-only domain freezes its Reflection: every write is refused
/// with the domain's own sentence and nothing on disk moves; reads answer.
#[tokio::test]
async fn a_read_only_domain_freezes_its_reflection() {
    let fx = crate::docs::reflecting().await;
    let page = "# Deploying\n\n## Night deploys\n\nDeploys run at night.\n";
    fx.engine
        .write_reflection(&write("guides/deploying.md", page), &person())
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                path: Some("guides/deploying.md".into()),
                section: Some("Night deploys".into()),
                text: Some("Still true?".into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    std::fs::write(
        fx.root.join("MANIFEST.md"),
        crate::fixture::manifest("reflection: on\nread_only: on\n"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let before = std::fs::read_to_string(fx.root.join("reflections/guides/deploying.md")).unwrap();
    let threads_before =
        std::fs::read_to_string(fx.root.join("reflections/guides/deploying.threads.md")).unwrap();
    let frozen = |err: crystalline_service::EngineError| {
        assert!(
            matches!(err, crystalline_service::EngineError::DomainReadOnly(_)),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "'eng' is read-only; its owner switches read_only off in the domain policies"
        );
    };
    frozen(
        fx.engine
            .write_reflection(&write("guides/deploying.md", "# Deploying\n"), &person())
            .await
            .unwrap_err(),
    );
    frozen(
        fx.engine
            .write_reflection(&write("guides/new.md", "# New\n"), &person())
            .await
            .unwrap_err(),
    );
    frozen(
        fx.engine
            .move_reflection("eng", "guides/deploying.md", "guides/moved.md", &person())
            .await
            .unwrap_err(),
    );
    frozen(
        fx.engine
            .delete_reflection("eng", "guides/deploying.md", &person())
            .await
            .unwrap_err(),
    );
    // Any section id: the freeze answers before the ids are looked at.
    frozen(
        fx.engine
            .settle_reflection(
                "eng",
                "guides/deploying.md",
                &["0000aaaa".to_string()],
                "kept",
                &person(),
            )
            .await
            .unwrap_err(),
    );
    let answer = DiscussParams {
        domain: "eng".into(),
        action: Some("answer".into()),
        path: Some("guides/deploying.md".into()),
        thread: opened["id"].as_str().map(str::to_string),
        text: Some("Yes.".into()),
        ..Default::default()
    };
    frozen(fx.engine.discuss(&answer, &person()).await.unwrap_err());
    let another = DiscussParams {
        domain: "eng".into(),
        path: Some("guides/deploying.md".into()),
        section: Some("Night deploys".into()),
        text: Some("Another one".into()),
        ..Default::default()
    };
    frozen(fx.engine.discuss(&another, &person()).await.unwrap_err());
    assert_eq!(
        std::fs::read_to_string(fx.root.join("reflections/guides/deploying.md")).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read_to_string(fx.root.join("reflections/guides/deploying.threads.md")).unwrap(),
        threads_before,
        "a refused answer and a refused open leave the threads file as it was"
    );
    assert!(!fx.root.join("reflections/guides/new.md").exists());
    let doc = fx
        .engine
        .read_reflection("eng", "guides/deploying.md", &person())
        .await
        .unwrap();
    assert!(
        doc["markdown"]
            .as_str()
            .unwrap()
            .contains("Deploys run at night."),
        "reads answer: {doc}"
    );
    fx.engine
        .read_reflection_tree("eng", &person())
        .await
        .unwrap();
    let listed = DiscussParams {
        domain: "eng".into(),
        action: Some("list".into()),
        ..Default::default()
    };
    fx.engine.discuss(&listed, &person()).await.unwrap();
}

/// A contributor at knowledge `none` cannot see the domain as engrams, yet a
/// read-only domain answers it with the domain's own sentence, not as an
/// unknown domain: the check runs unscoped after the reflection gate.
#[tokio::test]
async fn a_read_only_domain_answers_a_knowledge_none_contributor_with_its_sentence() {
    let (fx, ann) = crate::rights::contributor_engine().await;
    std::fs::write(
        fx.root.join("MANIFEST.md"),
        manifest("reflection: on\nread_only: on\n"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let err = fx
        .engine
        .write_reflection(
            &write("guides/a.md", "# A\n"),
            &ReflectionCaller::person(ann, "ann"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "'eng' is read-only; its owner switches read_only off in the domain policies"
    );
}

/// R18: an account whose write list leaves `eng` out keeps Read, so it
/// comments; answering a feedback item needs curate, and that refusal
/// names the list and its fix.
#[tokio::test]
async fn an_off_list_account_comments_and_cannot_answer_feedback() {
    use std::sync::Arc;

    use crystalline_service::rest::{AuthStore, Role};
    use crystalline_service::{DomainAccess, Scope};

    let fx = crate::docs::reflecting().await;
    fx.engine
        .write_reflection(&write("d.md", PAGE), &person())
        .await
        .unwrap();
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("ed", "ed", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.set_write_list("ed", &["agents-*".to_string()])
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let ed = ReflectionCaller::agent(
        Scope::User {
            account: "ed".into(),
            admin: false,
        },
        "ed",
        false,
    );
    let opened = fx
        .engine
        .discuss(&open("aaaa1111", None, "Still true?"), &ed)
        .await
        .unwrap();
    fx.engine
        .discuss(&step("answer", opened["id"].as_str().unwrap(), "Yes."), &ed)
        .await
        .unwrap();
    let page = std::fs::read_to_string(fx.root.join("reflections/d.md")).unwrap();
    let mut answering = open("aaaa1111", None, "I read your change.");
    answering.answers_feedback = true;
    let err = fx.engine.discuss(&answering, &ed).await.unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Forbidden(_)),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        crystalline_service::WriteBlock::WriteList {
            account: "ed".into()
        }
        .sentence("eng")
    );
    assert_eq!(
        std::fs::read_to_string(fx.root.join("reflections/d.md")).unwrap(),
        page
    );
}

/// A heading two sections share does not say which one a thread is on, so a
/// thread opened by it is refused with the way out, and the same section
/// opened by its id is not.
#[tokio::test]
async fn a_thread_on_a_heading_two_sections_share_is_refused_and_says_to_use_the_id() {
    let (fx, _vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    let page =
        "# D\n\n## Notes\n<!-- r:aaaa1111 -->\n\nOne.\n\n## Notes\n<!-- r:bbbb2222 -->\n\nTwo.\n";
    fx.engine
        .write_reflection(&write("d.md", page), &person())
        .await
        .unwrap();
    let err = fx
        .engine
        .discuss(&open("Notes", None, "Which one?"), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("names 2 sections"), "{err}");
    assert!(err.contains("by the section's id"), "{err}");
    let opened = fx
        .engine
        .discuss(&open("bbbb2222", Some("Two."), "This one."), &person())
        .await
        .unwrap();
    assert_eq!(opened["anchor"]["state"], "found", "{opened}");
}

/// Two sections a plain editor gave one heading have no ids yet: the
/// refusal says how to get them instead of pointing at ids that are not
/// there.
#[tokio::test]
async fn a_shared_heading_without_ids_says_how_to_get_them() {
    let (fx, _vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", "# D\n\nIntro.\n"), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/d.md");
    let page = std::fs::read_to_string(&path).unwrap() + "\n## Notes\n\nOne.\n\n## Notes\n\nTwo.\n";
    std::fs::write(&path, page).unwrap();
    fx.engine.sync(None).await.unwrap();
    let err = fx
        .engine
        .discuss(&open("Notes", None, "Which one?"), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("names 2 sections"), "{err}");
    assert!(err.contains("none of them has an id yet"), "{err}");
    assert!(err.contains("gives each heading its id"), "{err}");
}

/// When only some of the shared sections lack an id, the refusal says which
/// ones, by line, and that the others are opened by their id.
#[tokio::test]
async fn a_shared_heading_names_the_sections_without_an_id() {
    let (fx, _vera) = viewer_engine(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    fx.engine
        .write_reflection(&write("d.md", "# D\n\n## Notes\n\nOne.\n"), &person())
        .await
        .unwrap();
    let path = fx.root.join("reflections/d.md");
    let page = std::fs::read_to_string(&path).unwrap() + "\n## Notes\n\nTwo.\n";
    std::fs::write(&path, &page).unwrap();
    fx.engine.sync(None).await.unwrap();
    let line = page
        .lines()
        .enumerate()
        .filter(|(_, l)| *l == "## Notes")
        .map(|(n, _)| n + 1)
        .last()
        .unwrap();
    let err = fx
        .engine
        .discuss(&open("Notes", None, "Which one?"), &person())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(&format!("the one at line {line} has no id yet")),
        "{err}"
    );
    assert!(!err.contains("none of them"), "{err}");
    assert!(
        err.contains("for the others, open it by the section's id"),
        "{err}"
    );
}
