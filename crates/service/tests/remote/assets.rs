//! An agent's file upload and download through the remote `tool` door: the
//! verb cores run here exactly as over MCP, so a connected machine's agent
//! uploads an engram attachment with `write_engram` and `path`, a Reflection
//! file with `write_reflection` `write_asset`, and reads either back, under
//! the token's account. Files stay small here: the ctl body cap is the
//! transport's business.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crystalline_core::automation::Loop;
use crystalline_service::Scope;
use serde_json::{Value, json};

use crate::fixture::{Options, RemoteServer};

fn tool(name: &str, args: Value) -> Value {
    json!({ "v": 1, "cmd": "tool", "tool": name, "args": args })
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00upload\xff\xfe\x00";

fn keeper() -> Scope {
    Scope::User {
        account: "keeper".into(),
        admin: false,
    }
}

#[tokio::test]
async fn a_text_file_goes_up_and_comes_back_as_text() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, up) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_engram",
                json!({ "domain": "open", "path": "assets/flow.svg", "content": "<svg/>" }),
            ),
        )
        .await;
    assert_eq!(up["ok"], true, "{up}");
    let receipt = &up["data"];
    assert_eq!(receipt["domain"], "open", "{receipt}");
    assert_eq!(receipt["path"], "assets/flow.svg", "{receipt}");
    assert_eq!(receipt["mime"], "image/svg+xml", "{receipt}");
    assert_eq!(receipt["size"], 6, "{receipt}");
    assert_eq!(receipt["created"], true, "{receipt}");
    assert!(receipt.get("draft").is_none(), "{receipt}");
    assert_eq!(
        std::fs::read(server.file("open", "assets/flow.svg")).unwrap(),
        b"<svg/>"
    );

    let (_, down) = server
        .ctl(
            Some(&keeper),
            tool(
                "read_engram",
                json!({ "identifier": "assets/flow.svg", "domain": "open" }),
            ),
        )
        .await;
    assert_eq!(down["ok"], true, "{down}");
    let file = &down["data"];
    assert_eq!(file["text"], "<svg/>", "{file}");
    assert!(file.get("base64").is_none(), "{file}");
    assert_eq!(file["sha256"], receipt["sha256"], "{file}");
    assert_eq!(file["mime"], "image/svg+xml", "{file}");
    assert_eq!(file["path"], "assets/flow.svg", "{file}");
}

#[tokio::test]
async fn a_binary_file_goes_up_as_base64_and_comes_back_byte_for_byte() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, up) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_engram",
                json!({ "domain": "open", "path": "assets/shot.png", "base64": BASE64.encode(PNG) }),
            ),
        )
        .await;
    assert_eq!(up["ok"], true, "{up}");
    let (_, down) = server
        .ctl(
            Some(&keeper),
            tool(
                "read_engram",
                json!({ "identifier": "./assets/shot.png", "domain": "open" }),
            ),
        )
        .await;
    assert_eq!(down["ok"], true, "{down}");
    let bytes = BASE64
        .decode(down["data"]["base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, PNG);
    assert_eq!(down["data"]["sha256"], up["data"]["sha256"]);
}

#[tokio::test]
async fn a_reader_is_refused_and_an_editor_uploads() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let looker = server.token_for("looker").await;
    let args = json!({ "domain": "open", "path": "assets/a.png", "base64": BASE64.encode(PNG) });
    let (_, reply) = server
        .ctl(Some(&looker), tool("write_engram", args.clone()))
        .await;
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(!server.file("open", "assets/a.png").exists());
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server.ctl(Some(&keeper), tool("write_engram", args)).await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(server.file("open", "assets/a.png").exists());
}

/// Review Focus 4: an upload into a domain that reviews changes lands in the
/// uploader's own draft and says so; the folder the team reviewed is not
/// touched.
#[tokio::test]
async fn an_upload_in_review_mode_is_a_draft_and_the_tree_is_unchanged() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_engram",
                json!({ "domain": "team", "path": "assets/plan.png", "base64": BASE64.encode(PNG) }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["data"]["draft"], true, "{reply}");
    assert!(
        !server.file("team", "assets/plan.png").exists(),
        "the reviewed folder holds no file"
    );
    let (_, down) = server
        .ctl(
            Some(&keeper),
            tool(
                "read_engram",
                json!({ "identifier": "assets/plan.png", "domain": "team" }),
            ),
        )
        .await;
    assert_eq!(
        down["ok"], true,
        "keeper reads their own draft file: {down}"
    );
    let out = server.token_for("out").await;
    let (_, theirs) = server
        .ctl(
            Some(&out),
            tool(
                "read_engram",
                json!({ "identifier": "assets/plan.png", "domain": "team" }),
            ),
        )
        .await;
    assert_eq!(theirs["ok"], false, "nobody else sees it: {theirs}");
}

/// An upload with a share-link follows the join into the draft owner's
/// files, as the REST upload with `X-Crystalline-Join` does; a delete has no
/// join to follow and is screened as REST screens one without the header, so
/// it never reaches into the owner's files.
#[tokio::test]
async fn an_upload_follows_a_share_link_and_a_delete_stays_the_callers_own() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server
        .engine
        .write_engram_as(
            &serde_json::from_value(json!({
                "domain": "team",
                "title": "Plan",
                "content": "- [fact] keeper drafted this\n\n![sketch](assets/sketch.png)\n"
            }))
            .unwrap(),
            None,
            &keeper(),
        )
        .await
        .unwrap();
    let link = server
        .auth
        .mint_overlay_grant("team", "plan.md", "keeper", None)
        .await
        .unwrap()
        .token;
    let out = server.token_for("out").await;
    let (_, up) = server
        .ctl(
            Some(&out),
            tool(
                "write_engram",
                json!({
                    "domain": "team",
                    "path": "assets/sketch.png",
                    "base64": BASE64.encode(PNG),
                    "share_link": link,
                }),
            ),
        )
        .await;
    assert_eq!(up["ok"], true, "{up}");
    assert_eq!(up["data"]["draft"], true, "{up}");
    let (bytes, _) = server
        .engine
        .attachment_read_as("team", "assets/sketch.png", &keeper())
        .await
        .expect("the file is in keeper's draft files");
    assert_eq!(bytes, PNG);

    let (_, gone) = server
        .ctl(
            Some(&out),
            tool(
                "delete_engram",
                json!({ "identifier": "assets/sketch.png", "domain": "team" }),
            ),
        )
        .await;
    assert_eq!(
        gone["ok"], false,
        "out holds no such file of their own: {gone}"
    );
    let (bytes, _) = server
        .engine
        .attachment_read_as("team", "assets/sketch.png", &keeper())
        .await
        .expect("keeper's draft file stands");
    assert_eq!(bytes, PNG);
}

#[tokio::test]
async fn a_reflection_file_goes_up_and_comes_back_through_the_reflection_verbs() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let keeper = server.token_for("keeper").await;
    let (_, up) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_reflection",
                json!({
                    "domain": "open",
                    "action": "write_asset",
                    "path": "assets/up.png",
                    "base64": BASE64.encode(PNG),
                }),
            ),
        )
        .await;
    assert_eq!(up["ok"], true, "{up}");
    assert_eq!(up["data"]["path"], "assets/up.png", "{up}");
    assert_eq!(up["data"]["created"], true, "{up}");
    assert_eq!(up["data"]["mime"], "image/png", "{up}");
    let (_, down) = server
        .ctl(
            Some(&keeper),
            tool(
                "read_reflection",
                json!({ "domain": "open", "path": "assets/up.png" }),
            ),
        )
        .await;
    assert_eq!(down["ok"], true, "{down}");
    assert_eq!(
        BASE64
            .decode(down["data"]["base64"].as_str().unwrap())
            .unwrap(),
        PNG
    );
    assert_eq!(down["data"]["sha256"], up["data"]["sha256"]);
}

/// The paths the evolve lease on `open` has noted, sorted.
async fn noted(server: &RemoteServer) -> Vec<String> {
    let store = server.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("open").await.unwrap().unwrap();
    let mut written = store
        .automation_lease(id, "evolve")
        .await
        .unwrap()
        .expect("the lease is held")
        .written;
    written.sort();
    written
}

/// A headless run shares what its runner wrote over MCP: an uploaded file is
/// noted under its path in the domain, an engram attachment at `assets/...`
/// and a Reflection file at `reflections/assets/...`, for a move and a
/// delete of one too.
#[tokio::test]
async fn an_upload_by_the_lease_account_is_noted_for_the_run() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server
        .engine
        .automation_lease_take(Loop::Evolve, "open", "k8s", &keeper())
        .await
        .unwrap();
    let token = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&token),
            tool(
                "write_engram",
                json!({ "domain": "open", "path": "assets/run.txt", "content": "ran" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let (_, reply) = server
        .ctl(
            Some(&token),
            tool(
                "write_reflection",
                json!({ "domain": "open", "action": "write_asset", "path": "assets/run.png", "base64": BASE64.encode(PNG) }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        noted(&server).await,
        vec!["assets/run.txt", "reflections/assets/run.png"]
    );
    let (_, reply) = server
        .ctl(
            Some(&token),
            tool(
                "write_reflection",
                json!({ "domain": "open", "action": "move_asset", "path": "assets/run.png", "to": "assets/moved.png" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let (_, reply) = server
        .ctl(
            Some(&token),
            tool(
                "write_reflection",
                json!({ "domain": "open", "action": "delete_asset", "path": "assets/pic.png" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        noted(&server).await,
        vec![
            "assets/run.txt",
            "reflections/assets/moved.png",
            "reflections/assets/pic.png",
            "reflections/assets/run.png",
        ]
    );
}

/// A root engram called `assets` is knowledge through the remote door too.
#[tokio::test]
async fn a_root_engram_named_assets_is_an_engram_through_the_remote_door() {
    let server = RemoteServer::start(Options::TOKENS).await;
    std::fs::write(
        server.file("open", "assets.md"),
        crate::fixture::engram("Assets", "assets", "what the team owns"),
    )
    .unwrap();
    server.engine.sync(Some("open")).await.unwrap();
    let keeper = server.token_for("keeper").await;
    for identifier in ["assets", "Assets"] {
        let (_, reply) = server
            .ctl(
                Some(&keeper),
                tool(
                    "read_engram",
                    json!({ "identifier": identifier, "domain": "open" }),
                ),
            )
            .await;
        assert_eq!(reply["ok"], true, "{identifier}: {reply}");
        assert_eq!(reply["data"]["permalink"], "assets", "{reply}");
    }
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool(
                "delete_engram",
                json!({ "identifier": "assets", "domain": "open" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(reply["data"].get("attachment").is_none(), "{reply}");
    assert!(!server.file("open", "assets.md").exists());
}

/// A body is read up to `MAX_TOOL_BODY_BYTES` (15 MiB): a 10 MiB file as base64
/// (13,981,016 bytes) and the envelope fit, and a body one byte over the cap
/// is refused by the door before a verb runs.
#[tokio::test]
async fn a_full_size_file_fits_the_ctl_body_and_one_byte_over_the_cap_is_refused() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let cap = crystalline_core::MAX_ATTACHMENT_BYTES as usize;
    let (_, up) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_engram",
                json!({ "domain": "open", "path": "assets/big.pdf", "base64": BASE64.encode(vec![7u8; cap]) }),
            ),
        )
        .await;
    assert_eq!(up["ok"], true, "{up}");
    assert_eq!(up["data"]["size"], cap, "{up}");
    assert_eq!(
        std::fs::metadata(server.file("open", "assets/big.pdf"))
            .unwrap()
            .len(),
        cap as u64
    );

    let limit = crystalline_service::rest::MAX_TOOL_BODY_BYTES;
    let with_padding =
        |pad: usize| tool("search_engrams", json!({ "query": "x".repeat(pad) })).to_string();
    let envelope = with_padding(0).len();
    let at_the_limit = with_padding(limit - envelope);
    assert_eq!(at_the_limit.len(), limit);
    let post = |body: String| {
        let request = server
            .http
            .post(format!("{}/api/v1/ctl", server.base()))
            .header("content-type", "application/json")
            .bearer_auth(&keeper)
            .body(body);
        async move {
            let text = request.send().await.unwrap().text().await.unwrap();
            serde_json::from_str::<Value>(&text).unwrap()
        }
    };
    let served = post(at_the_limit).await;
    assert!(
        served["error"]
            .as_str()
            .is_none_or(|e| !e.starts_with("invalid body")),
        "a body of exactly the cap is read: {served}"
    );
    let refused = post(with_padding(limit - envelope + 1)).await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid body"),
        "{refused}"
    );
}
