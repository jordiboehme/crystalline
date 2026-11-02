//! **An agent's upload and download of files over MCP (stdio).**
//!
//! `write_engram` with `path` stores an engram attachment and `read_engram`
//! with an `assets/` identifier hands it back; `write_reflection`
//! `write_asset` and `read_reflection` with an `assets/` path do the same for
//! a Reflection file. Both run the engine entry points REST runs, so a bad
//! path is refused in REST's words and nothing is written anywhere.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::engine::ASSET_EXISTS;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00upload\xff\xfe\x00bytes";

fn manifest(extra: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n{extra}---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n"
    )
}

struct Harness {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    engine: Arc<Engine>,
}

impl Harness {
    /// A file domain `eng` with the Reflection on, `manifest_extra` added to
    /// its MANIFEST, on an instance that is read-only when `read_only`.
    async fn with(manifest_extra: &str, read_only: bool) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = root.join("eng");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            manifest(&format!("reflection: on\n{manifest_extra}")),
        )
        .unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let token_store = root.join("token-store");
        std::fs::create_dir_all(&token_store).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_token_store_dir(token_store)
                .with_read_only(read_only),
        );
        engine.sync(None).await.unwrap();
        Harness {
            _tmp: tmp,
            root,
            engine,
        }
    }

    async fn new() -> Harness {
        Harness::with("", false).await
    }

    async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let client = rmcp::serve_client((), client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }

    /// Every file under the domain's root, by its relative path.
    fn files(&self) -> BTreeSet<String> {
        let dir = self.root.join("eng");
        walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                e.path()
                    .strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }
}

/// A tool call: `Ok` with the whole result for a success, `Err` with the
/// message for a refusal, whether it came as a tool error or a protocol one.
async fn try_call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<Value, String> {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match peer.call_tool(params).await {
        Ok(result) => {
            let value = serde_json::to_value(result).unwrap();
            if value["isError"] == json!(true) {
                Err(value["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string())
            } else {
                Ok(value)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The JSON a successful call answered in its first text block.
fn body(result: &Value) -> Value {
    let text = result["content"][0]["text"].as_str().unwrap();
    let json = text.split("\n\n---\n").next().unwrap();
    serde_json::from_str(json).unwrap()
}

fn image_blocks(result: &Value) -> Vec<&Value> {
    result["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["type"] == "image")
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_file_round_trips_as_text_and_an_svg_may_come_as_base64() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let up = try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "path": "assets/flow.svg", "content": "<svg/>" }),
    )
    .await
    .unwrap();
    let receipt = body(&up);
    assert_eq!(receipt["created"], true, "{receipt}");
    assert_eq!(receipt["mime"], "image/svg+xml", "{receipt}");
    let down = try_call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "assets/flow.svg", "domain": "eng" }),
    )
    .await
    .unwrap();
    let file = body(&down);
    assert_eq!(file["text"], "<svg/>", "{file}");
    assert_eq!(file["sha256"], receipt["sha256"]);
    assert_eq!(file["size"], 6);
    assert!(image_blocks(&down).is_empty(), "an svg is no image block");

    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "path": "assets/b64.svg", "base64": BASE64.encode(svg) }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(h.root.join("eng/assets/b64.svg")).unwrap(),
        svg
    );
    let down = try_call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "assets/b64.svg", "domain": "eng" }),
    )
    .await
    .unwrap();
    assert_eq!(
        body(&down)["text"].as_str(),
        Some(std::str::from_utf8(svg).unwrap())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_picture_round_trips_byte_for_byte_and_comes_back_as_an_image_too() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "path": "assets/shot.png", "base64": BASE64.encode(PNG) }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(h.root.join("eng/assets/shot.png")).unwrap(),
        PNG
    );
    let down = try_call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "assets/shot.png", "domain": "eng" }),
    )
    .await
    .unwrap();
    let file = body(&down);
    assert!(
        file.get("base64").is_none(),
        "the bytes travel once, as the image: {file}"
    );
    assert!(file.get("text").is_none(), "{file}");
    assert!(
        file["note"].as_str().is_some_and(|n| n.contains("image")),
        "{file}"
    );
    assert_eq!(file["mime"], "image/png");
    assert_eq!(file["size"], PNG.len());
    assert!(file["sha256"].as_str().is_some());
    let images = image_blocks(&down);
    assert_eq!(images.len(), 1, "{down}");
    assert_eq!(images[0]["mimeType"], "image/png");
    assert_eq!(
        images[0]["data"].as_str(),
        Some(BASE64.encode(PNG).as_str())
    );

    let err = try_call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "assets/shot.png" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("domain"), "a file needs its domain: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_upload_is_refused_unless_it_overwrites() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let args = |b: &[u8], overwrite: bool| json!({ "domain": "eng", "path": "assets/a.png", "base64": BASE64.encode(b), "overwrite": overwrite });
    try_call(client.peer(), "write_engram", args(PNG, false))
        .await
        .unwrap();
    let err = try_call(client.peer(), "write_engram", args(b"other", false))
        .await
        .unwrap_err();
    assert!(err.contains(ASSET_EXISTS), "{err}");
    assert_eq!(std::fs::read(h.root.join("eng/assets/a.png")).unwrap(), PNG);
    let replaced = body(
        &try_call(client.peer(), "write_engram", args(b"other", true))
            .await
            .unwrap(),
    );
    assert_eq!(replaced["created"], false, "{replaced}");
    assert_eq!(
        std::fs::read(h.root.join("eng/assets/a.png")).unwrap(),
        b"other"
    );
}

/// Review Focus 1: a path that is not under `assets/`, or leaves it, is
/// refused with the sentence REST gives, and nothing is written anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_outside_assets_is_refused_in_rests_words_and_writes_nothing() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let before = h.files();
    for path in [
        "assets/../x.md",
        "./assets/a.png",
        "ASSETS/a.png",
        "Assets/a.png",
        "/assets/a.png",
        "/etc/a.png",
        "notes/a.png",
        "assets/a.md",
        "assets/.hidden.png",
        "assets/a/../../b.png",
        "assets\\a.png",
        "a.png",
    ] {
        let rest = h
            .engine
            .attachment_write_as(
                "eng",
                path,
                PNG.to_vec(),
                &crystalline_service::Scope::Unrestricted,
            )
            .await
            .unwrap_err()
            .to_string();
        let err = try_call(
            client.peer(),
            "write_engram",
            json!({ "domain": "eng", "path": path, "base64": BASE64.encode(PNG) }),
        )
        .await
        .unwrap_err();
        assert!(err.contains(&rest), "{path}: {err} / {rest}");
        let err = try_call(
            client.peer(),
            "write_reflection",
            json!({ "domain": "eng", "action": "write_asset", "path": path, "base64": BASE64.encode(PNG) }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("attachment path"), "{path}: {err}");
    }
    assert_eq!(h.files(), before, "nothing was written");
}

/// Review Focus 2: a payload that cannot be the file is refused before
/// anything is written, naming the field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bad_payload_is_refused_naming_the_field_and_writes_nothing() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let before = h.files();
    let cases = [
        (
            json!({ "domain": "eng", "path": "assets/a.png", "content": "not a picture" }),
            "base64",
        ),
        (
            json!({ "domain": "eng", "path": "assets/a.png", "base64": "@@ not base64 @@" }),
            "base64",
        ),
        (
            json!({ "domain": "eng", "path": "assets/a.txt", "content": "x", "base64": BASE64.encode(b"x") }),
            "not both",
        ),
        (
            json!({ "domain": "eng", "path": "assets/a.txt" }),
            "content",
        ),
        (
            json!({ "domain": "eng", "path": "assets/a.txt", "content": "x", "title": "A" }),
            "title",
        ),
        (
            json!({ "domain": "eng", "path": "assets/a.txt", "content": "x", "tags": ["t"] }),
            "tags",
        ),
        (
            json!({ "domain": "eng", "path": "assets/a.txt", "content": "x", "folder": "f" }),
            "folder",
        ),
    ];
    for (args, field) in &cases {
        let err = try_call(client.peer(), "write_engram", args.clone())
            .await
            .unwrap_err();
        assert!(err.contains(field), "{args}: {err}");
    }
    let err = try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "content": "x" }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("title"),
        "an engram still needs a title: {err}"
    );
    for (args, field) in [
        (
            json!({ "domain": "eng", "action": "write_asset", "path": "assets/a.png", "content": "nope" }),
            "base64",
        ),
        (
            json!({ "domain": "eng", "action": "write_asset", "path": "assets/a.png", "base64": "%%%" }),
            "base64",
        ),
        (
            json!({ "domain": "eng", "action": "write_asset", "path": "assets/a.png" }),
            "content",
        ),
    ] {
        let err = try_call(client.peer(), "write_reflection", args.clone())
            .await
            .unwrap_err();
        assert!(err.contains(field), "{args}: {err}");
    }
    assert_eq!(h.files(), before, "nothing was written");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_mib_is_stored_and_one_byte_more_is_refused() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let cap = crystalline_core::MAX_ATTACHMENT_BYTES as usize;
    let err = try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "path": "assets/big.pdf", "base64": BASE64.encode(vec![7u8; cap + 1]) }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains(&format!("over the {cap} byte ceiling")),
        "{err}"
    );
    assert!(!h.root.join("eng/assets/big.pdf").exists());
    let up = try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "path": "assets/big.pdf", "base64": BASE64.encode(vec![7u8; cap]) }),
    )
    .await
    .unwrap();
    assert_eq!(body(&up)["size"], cap);
    assert_eq!(
        std::fs::metadata(h.root.join("eng/assets/big.pdf"))
            .unwrap()
            .len(),
        cap as u64
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_and_a_read_only_domain_refuse_both_writes() {
    for h in [
        Harness::with("", true).await,
        Harness::with("read_only: on\n", false).await,
    ] {
        let (client, _server) = h.connect().await;
        let before = h.files();
        try_call(
            client.peer(),
            "write_engram",
            json!({ "domain": "eng", "path": "assets/a.png", "base64": BASE64.encode(PNG) }),
        )
        .await
        .unwrap_err();
        try_call(
            client.peer(),
            "write_reflection",
            json!({ "domain": "eng", "action": "write_asset", "path": "assets/a.png", "base64": BASE64.encode(PNG) }),
        )
        .await
        .unwrap_err();
        assert_eq!(h.files(), before, "nothing was written");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reflection_file_round_trips_through_the_reflection_verbs() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let up = body(
        &try_call(
            client.peer(),
            "write_reflection",
            json!({ "domain": "eng", "action": "write_asset", "path": "assets/data.csv", "content": "a,b\n1,2\n" }),
        )
        .await
        .unwrap(),
    );
    assert_eq!(up["created"], true, "{up}");
    assert_eq!(up["mime"], "text/csv", "{up}");
    assert_eq!(
        std::fs::read_to_string(h.root.join("eng/reflections/assets/data.csv")).unwrap(),
        "a,b\n1,2\n"
    );
    let err = try_call(
        client.peer(),
        "write_reflection",
        json!({ "domain": "eng", "action": "write_asset", "path": "assets/data.csv", "content": "x" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains(ASSET_EXISTS), "{err}");
    let replaced = body(
        &try_call(
            client.peer(),
            "write_reflection",
            json!({ "domain": "eng", "action": "write_asset", "path": "assets/data.csv", "content": "x\n", "overwrite": true }),
        )
        .await
        .unwrap(),
    );
    assert_eq!(replaced["created"], false, "{replaced}");
    let down = try_call(
        client.peer(),
        "read_reflection",
        json!({ "domain": "eng", "path": "assets/data.csv" }),
    )
    .await
    .unwrap();
    let file = body(&down);
    assert_eq!(file["text"], "x\n", "{file}");
    assert_eq!(file["sha256"], replaced["sha256"]);
    assert_eq!(file["path"], "assets/data.csv");
}

/// A binary file that is no picture comes back as base64 and with no image.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pdf_comes_back_as_base64_without_an_image() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let pdf = b"%PDF-1.4 \xff\x00 bytes";
    try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "path": "assets/deck.pdf", "base64": BASE64.encode(pdf) }),
    )
    .await
    .unwrap();
    let down = try_call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "assets/deck.pdf", "domain": "eng" }),
    )
    .await
    .unwrap();
    let file = body(&down);
    assert_eq!(
        BASE64.decode(file["base64"].as_str().unwrap()).unwrap(),
        pdf
    );
    assert!(file.get("note").is_none(), "{file}");
    assert!(image_blocks(&down).is_empty(), "{down}");
}

const ASSETS_ENGRAM: &str = "---\ntype: engram\ntitle: Assets\npermalink: assets\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Assets\n\n- [fact] what the team owns\n";

/// A root engram called `assets` is knowledge, not the folder: it is read
/// and deleted by its permalink and its title as any engram is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_engram_named_assets_is_read_and_deleted_as_an_engram() {
    let h = Harness::new().await;
    std::fs::write(h.root.join("eng/assets.md"), ASSETS_ENGRAM).unwrap();
    h.engine.sync(None).await.unwrap();
    let (client, _server) = h.connect().await;
    for identifier in ["assets", "Assets"] {
        let read = body(
            &try_call(
                client.peer(),
                "read_engram",
                json!({ "identifier": identifier, "domain": "eng" }),
            )
            .await
            .unwrap(),
        );
        assert_eq!(read["permalink"], "assets", "{identifier}: {read}");
        assert!(read.get("sha256").is_none(), "{read}");
    }
    let gone = body(
        &try_call(
            client.peer(),
            "delete_engram",
            json!({ "identifier": "assets", "domain": "eng" }),
        )
        .await
        .unwrap(),
    );
    assert!(gone.get("attachment").is_none(), "an engram delete: {gone}");
    assert!(!h.root.join("eng/assets.md").exists());
}

/// An engram needs its `content` field, but an empty body is accepted as it
/// always was; a file's text may be empty too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engram_needs_content_but_an_empty_body_is_accepted_and_an_empty_text_file_uploads() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let err = try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "title": "No Body" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("content"), "{err}");
    assert!(!h.root.join("eng/no-body.md").exists());
    try_call(
        client.peer(),
        "write_engram",
        json!({ "domain": "eng", "title": "Empty Body", "content": "" }),
    )
    .await
    .unwrap();
    assert!(h.root.join("eng/empty-body.md").exists());
    let up = body(
        &try_call(
            client.peer(),
            "write_engram",
            json!({ "domain": "eng", "path": "assets/empty.txt", "content": "" }),
        )
        .await
        .unwrap(),
    );
    assert_eq!(up["size"], 0, "{up}");
    assert_eq!(
        std::fs::read(h.root.join("eng/assets/empty.txt")).unwrap(),
        b""
    );
    let up = body(
        &try_call(
            client.peer(),
            "write_engram",
            json!({ "domain": "eng", "path": "assets/empty.csv", "base64": "" }),
        )
        .await
        .unwrap(),
    );
    assert_eq!(up["size"], 0, "{up}");
}
