//! A Reflection answer carries the display name beside every person it
//! names, resolved when the answer is built, while the stored thread keeps
//! the login name.

use serde_json::{Value, json};

use crate::rest::{Session, serve};

#[tokio::test]
async fn members_and_an_engram_read_carry_display_names() {
    let s = serve().await;
    s.auth.set_display("keeper", Some("Kee Per")).await.unwrap();
    s.auth.set_display("ed", Some("Ed Itor")).await.unwrap();
    let keeper = Session::login(s.addr, "keeper").await;
    let (status, members) = keeper.get("/api/v1/domains/eng/members").await;
    assert_eq!(status, 200, "{members}");
    assert_eq!(members["owner"], "keeper", "{members}");
    assert_eq!(members["owner_display"], "Kee Per", "{members}");
    let rows = members["members"].as_array().unwrap();
    let ed = rows.iter().find(|m| m["principal"] == "ed").unwrap();
    assert_eq!(ed["principal_display"], "Ed Itor", "{members}");
    let ann = rows.iter().find(|m| m["principal"] == "ann").unwrap();
    assert!(ann.get("principal_display").is_none(), "{members}");

    std::fs::write(
        s.fx.root.join("signed.md"),
        "---\ntype: engram\ntitle: Signed\npermalink: signed\nstatus: stable\n\
         generated:\n  by: human:ed\n  at: 2026-11-20\n\
         verified:\n  - by: human:keeper\n    at: 2026-11-21\n  - by: claude/1.0\n    at: 2026-11-22\n\
         ---\n\n# Signed\n\nText.\n",
    )
    .unwrap();
    s.fx.engine.sync(None).await.unwrap();
    let (status, read) = keeper.get("/api/v1/domains/eng/engrams/signed").await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(
        read["actor_display"],
        json!({ "human:ed": "Ed Itor", "human:keeper": "Kee Per" }),
        "{read}"
    );
}

fn author_of(thread: &Value, n: usize) -> &Value {
    &thread["entries"][n]["author"]
}

#[tokio::test]
async fn thread_answers_name_people_by_their_display_name() {
    let s = serve().await;
    s.auth.set_display("vera", Some("Vera Viewer")).await.unwrap();
    let vera = Session::login(s.addr, "vera").await;
    let (status, opened) = vera
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/threads",
            Some(json!({ "path": "guides/deploying.md", "section": "Night deploys",
                         "quote": "at night", "text": "Still?" })),
        )
        .await;
    assert_eq!(status, 201, "{opened}");
    assert_eq!(author_of(&opened, 0)["name"], "vera", "{opened}");
    assert_eq!(author_of(&opened, 0)["display"], "Vera Viewer", "{opened}");
    let id = opened["id"].as_str().unwrap().to_string();

    // `ann`'s display name is her login: no `display` is sent for her.
    let ann = Session::login(s.addr, "ann").await;
    let (status, answered) = ann
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/domains/eng/reflection/threads/{id}/answer"),
            Some(json!({ "path": "guides/deploying.md", "text": "Yes." })),
        )
        .await;
    assert_eq!(status, 200, "{answered}");
    assert_eq!(author_of(&answered, 1)["name"], "ann", "{answered}");
    assert!(author_of(&answered, 1).get("display").is_none(), "{answered}");
    assert_eq!(author_of(&answered, 0)["display"], "Vera Viewer");

    let (_, listed) = vera.get("/api/v1/domains/eng/reflection/threads").await;
    assert_eq!(
        author_of(&listed["threads"][0], 0)["display"],
        "Vera Viewer",
        "{listed}"
    );
    let (_, page) = vera
        .get("/api/v1/domains/eng/reflections/guides/deploying.md")
        .await;
    assert_eq!(
        author_of(&page["threads"][0], 0)["display"],
        "Vera Viewer",
        "{page}"
    );
    let (_, review) = vera.get("/api/v1/domains/eng/reflection/review").await;
    let waiting = review["waiting_for_the_agent"][0]["threads"][0].clone();
    assert_eq!(author_of(&waiting, 0)["display"], "Vera Viewer", "{review}");
    let (_, summary) = vera.get("/api/v1/domains/eng/reflection/summary").await;
    let people = &summary["open_by_page"][0]["threads"][0]["people"];
    let vera_row = people
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "vera")
        .unwrap_or_else(|| panic!("{summary}"));
    assert_eq!(vera_row["display"], "Vera Viewer");

    // A rename shows at once, and the stored thread keeps the login.
    s.auth.set_display("vera", Some("Vera King")).await.unwrap();
    let (_, listed) = vera.get("/api/v1/domains/eng/reflection/threads").await;
    assert_eq!(author_of(&listed["threads"][0], 0)["display"], "Vera King");
    assert_eq!(author_of(&listed["threads"][0], 0)["name"], "vera");
}
