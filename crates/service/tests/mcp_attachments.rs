//! **The MCP read surface for attachments, over both transports.**
//!
//! An attachment is a file a human added to a domain - a screenshot, a slide
//! deck, a data file - and the surface an agent reaches it through is three
//! things: one resource template that says how attachments are addressed, a
//! `resources/read` that hands back the bytes in the shape the mime asks for,
//! and the resource links `read_engram` appends for the attachments an engram
//! actually references.
//!
//! The division of labour those three make is the thing worth pinning: a tool
//! result carries **links**, never bytes, so a model spends context on a
//! screenshot only after deciding it needs to look at it, and `resources/read`
//! is the only place base64 is ever emitted.
//!
//! Both transports run here rather than one, because they are different code
//! paths in rmcp: the stdio leg drives a real client over a duplex pair, the
//! HTTP leg posts raw JSON-RPC at the daemon's own router with the era's
//! `_meta` and standard headers, exactly as `tests/mcp_modern_era.rs` does.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// The era this file's HTTP leg speaks, spelled once.
const ERA: &str = "2026-07-28";

/// A stand-in for an image: the bytes never have to decode, only to survive a
/// base64 round trip unchanged, so this is a short binary blob carrying the
/// NUL bytes and the invalid UTF-8 that would break a text-shaped path.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00shot\xff\xfe\x00bytes";

/// A text attachment, which comes back as text rather than base64.
const JSON: &str = "{\n  \"quarter\": \"Q3\",\n  \"rows\": 12\n}\n";

/// A text attachment whose bytes are Latin-1 rather than UTF-8. The mime says
/// text and the bytes say otherwise, which is the one case
/// `attachment_contents` decides in favour of the bytes.
const LATIN1: &[u8] = b"Gr\xf6\xdfe: 12 Zeilen\n";

/// An attachment whose name a person chose in their own language, so its uri
/// cannot be spelled without percent-encoding by a conforming client. Stored
/// under the raw name; asked for both ways.
const UMLAUT_PATH: &str = "assets/größe.png";

/// [`UMLAUT_PATH`] the way RFC 3986 makes a client spell it.
const UMLAUT_ENCODED: &str = "crystalline://eng/assets/gr%C3%B6%C3%9Fe.png";

/// An engram referencing two attachments that exist, one that does not, one
/// inside a fenced example and one absolute URL. Only the first two may ever
/// produce a resource link.
const SHOTS: &str = r#"---
type: engram
title: Shots
permalink: shots
tags:
  - eng
status: stable
recorded_at: 2026-01-01
---

# Shots

The dashboard as it looked in Q3:

![The dashboard](assets/shot.png)

The numbers behind it are in [the export](assets/deep/data.json), and the
same shot again as ![a repeat](assets/shot.png) must not link twice.

A reference nobody ever uploaded: [the missing one](assets/gone.png).

A remote image is somebody else's file: ![remote](https://example.com/x.png).

```markdown
![never counted](assets/fenced.png)
```
"#;

// --- the engine, and the two wires ------------------------------------------

struct Harness {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    engine: Arc<Engine>,
}

impl Harness {
    /// A file domain `eng` holding the MANIFEST, the referencing engram and
    /// four real attachments: one binary and one text (the text one nested a
    /// folder deep, so the template's `{+path}` expansion has something to
    /// prove), one text file whose bytes are not UTF-8 and one whose name needs
    /// percent-encoding to travel as a uri.
    ///
    /// The last two are deliberately unreferenced by any engram. They exist to
    /// be fetched by uri, which is a thing `resources/read` does without asking
    /// whether anything points at them; the orphan invariant is evolve's to
    /// enforce, not this surface's.
    async fn new() -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        let dir = root.join("eng");
        std::fs::create_dir_all(dir.join("assets/deep")).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
        )
        .unwrap();
        std::fs::write(dir.join("shots.md"), SHOTS).unwrap();
        std::fs::write(dir.join("assets/shot.png"), PNG).unwrap();
        std::fs::write(dir.join("assets/deep/data.json"), JSON).unwrap();
        std::fs::write(dir.join("assets/latin1.txt"), LATIN1).unwrap();
        std::fs::write(dir.join(UMLAUT_PATH), PNG).unwrap();
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
                .with_token_store_dir(token_store),
        );
        engine.sync(None).await.unwrap();
        Harness {
            _tmp: tmp,
            root,
            engine,
        }
    }

    /// A real rmcp client over a duplex pair: the stdio transport, with typed
    /// results rather than hand-parsed bytes.
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

    /// The daemon's real HTTP router on an ephemeral loopback port.
    async fn http(&self) -> std::net::SocketAddr {
        let auth = Arc::new(
            crystalline_service::rest::AuthStore::open(&self.root.join("web-auth.db"))
                .await
                .unwrap(),
        );
        let router = http_router(
            self.engine.clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            &[],
            auth,
            None,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap();
        });
        addr
    }
}

/// Call a tool and return the whole result as JSON, content blocks included.
async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Value {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    serde_json::to_value(peer.call_tool(params).await.unwrap()).unwrap()
}

/// The resource-link blocks in a tool result, in order.
fn resource_links(result: &Value) -> Vec<&Value> {
    result["content"]
        .as_array()
        .expect("a tool result carries content blocks")
        .iter()
        .filter(|block| block["type"] == json!("resource_link"))
        .collect()
}

// --- stdio ------------------------------------------------------------------

/// **The template says how every attachment is addressed.**
///
/// A template rather than a resource listing because the set is open and per
/// domain: enumerating every screenshot of every registered domain would spend
/// a client's context on files it will never open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_attachment_template_is_listed_with_its_reserved_path_expansion() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;

    let listed = client
        .peer()
        .list_resource_templates(Default::default())
        .await
        .unwrap()
        .resource_templates;
    assert_eq!(listed.len(), 1, "one template: {listed:?}");
    assert_eq!(
        listed[0].uri_template,
        "crystalline://{domain}/assets/{+path}"
    );
    assert_eq!(listed[0].name, "attachment");
    let description = listed[0].description.as_deref().unwrap_or_default();
    assert!(
        description.contains("attachment") && description.contains("resource links"),
        "the description says when to reach for it: {description}"
    );
}

/// **A binary attachment comes back base64, a text one as text.**
///
/// The mime decides the shape, not the caller: `is_text_attachment_mime` is
/// the same rule the REST file routes read, so an agent and a browser are told
/// the same thing about the same file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_reads_back_as_blob_or_text_by_its_mime() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let png = peer
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://eng/assets/shot.png",
        ))
        .await
        .unwrap();
    let contents = serde_json::to_value(&png.contents[0]).unwrap();
    assert_eq!(contents["mimeType"].as_str(), Some("image/png"));
    assert!(
        contents["text"].is_null(),
        "an image is never inlined as text: {contents}"
    );
    let blob = contents["blob"].as_str().expect("base64 bytes");
    assert_eq!(blob, BASE64.encode(PNG), "the exact bytes, base64 encoded");
    assert_eq!(
        BASE64.decode(blob).unwrap(),
        PNG,
        "and they decode back byte for byte"
    );

    // The nested path is the `{+path}` expansion doing its job: two segments,
    // one uri.
    let data = peer
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://eng/assets/deep/data.json",
        ))
        .await
        .unwrap();
    let contents = serde_json::to_value(&data.contents[0]).unwrap();
    assert_eq!(contents["mimeType"].as_str(), Some("application/json"));
    assert_eq!(contents["text"].as_str(), Some(JSON));
    assert!(
        contents["blob"].is_null(),
        "a text mime is never base64: {contents}"
    );
}

/// **A percent-encoded uri reaches the same file as the raw one.**
///
/// A filename a person chose in their own language cannot be spelled in an RFC
/// 3986 path without encoding, so a conforming client sends back something
/// other than the bytes we handed it. The REST file route gets the decoding
/// step for free from axum's `Path` extractor; without the same step here a
/// browser and an agent following the same link would disagree about whether
/// the file exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_percent_encoded_attachment_uri_reaches_the_same_file_as_the_raw_one() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    for uri in [
        UMLAUT_ENCODED,
        "crystalline://eng/assets/größe.png",
        // Encoding a character that never needed it is still a valid spelling
        // of the same path, and decoding is what makes the two agree.
        "crystalline://eng/assets/gr%C3%B6%C3%9Fe%2Epng",
    ] {
        let read = peer
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .unwrap_or_else(|e| panic!("{uri} names a stored attachment: {e}"));
        let contents = serde_json::to_value(&read.contents[0]).unwrap();
        assert_eq!(contents["mimeType"].as_str(), Some("image/png"), "{uri}");
        assert_eq!(
            contents["blob"].as_str(),
            Some(BASE64.encode(PNG).as_str()),
            "{uri} reads the same bytes as every other spelling"
        );
        assert_eq!(
            contents["uri"].as_str(),
            Some(uri),
            "the answer echoes the uri that was asked for, not a rewritten one"
        );
    }
}

/// Decoding is not lenient: a sequence that is not UTF-8 once decoded, and one
/// that decodes to a control character, are both refused rather than passed to
/// the filesystem. The NUL case matters most - a NUL reaching a filesystem call
/// truncates the name it is part of.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_or_control_bearing_percent_sequence_is_refused() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let not_utf8 = peer
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://eng/assets/%FF%FE.png",
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(not_utf8.contains("-32602"), "{not_utf8}");
    assert!(
        not_utf8.contains("UTF-8"),
        "the refusal says what is wrong with it: {not_utf8}"
    );

    let nul = peer
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://eng/assets/shot%00.png",
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(nul.contains("-32602"), "{nul}");
    assert!(
        nul.contains("control character"),
        "a decoded NUL never reaches a filesystem call: {nul}"
    );
}

/// **A text mime whose bytes are not UTF-8 falls back to the blob shape.**
///
/// The mime describes the file and the bytes are the file; when they disagree
/// the bytes win, because a lossy conversion would hand the caller something
/// that is not what is stored. A client that decodes the base64 gets the
/// Latin-1 bytes verbatim, and the mime still says what the file claims to be.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_attachment_that_is_not_utf8_comes_back_as_a_blob() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;

    let read = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://eng/assets/latin1.txt",
        ))
        .await
        .expect("a text mime with awkward bytes is served, not refused");
    let contents = serde_json::to_value(&read.contents[0]).unwrap();
    assert_eq!(
        contents["mimeType"].as_str(),
        Some("text/plain"),
        "the mime still describes the file: {contents}"
    );
    assert!(
        contents["text"].is_null(),
        "no lossy conversion happened: {contents}"
    );
    let blob = contents["blob"].as_str().expect("base64 bytes");
    assert_eq!(
        BASE64.decode(blob).unwrap(),
        LATIN1,
        "the bytes survive verbatim"
    );
}

/// An attachment nobody stored, and a domain nobody registered, are both
/// `invalid_params` with a message naming what was asked for - the same shape
/// an unknown skill uri gets, and the same shape the engine reports a missing
/// engram in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_attachment_or_domain_is_refused_with_invalid_params() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let missing = peer
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://eng/assets/gone.png",
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("-32602"),
        "an absent attachment is a parameter error: {missing}"
    );
    assert!(
        missing.contains("assets/gone.png"),
        "and it names the path: {missing}"
    );

    let unknown_domain = peer
        .read_resource(ReadResourceRequestParams::new(
            "crystalline://nosuch/assets/shot.png",
        ))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        unknown_domain.contains("nosuch"),
        "an unregistered domain names itself: {unknown_domain}"
    );

    // A uri that is neither a skill nor an attachment names both surfaces.
    let neither = peer
        .read_resource(ReadResourceRequestParams::new("crystalline://eng/shots"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        neither.contains("skill://crystalline-routing/SKILL.md")
            && neither.contains("crystalline://{domain}/assets/{+path}"),
        "the refusal names what this server does serve: {neither}"
    );
}

/// The skill resources are untouched by any of it: the same uris read back the
/// same bytes they did before attachments existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_skill_resource_still_reads_back_unchanged() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;

    let read = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(
            "skill://crystalline-intelligence/SKILL.md",
        ))
        .await
        .unwrap();
    let contents = serde_json::to_value(&read.contents[0]).unwrap();
    assert_eq!(contents["mimeType"].as_str(), Some("text/markdown"));
    assert_eq!(
        contents["text"].as_str(),
        Some(
            crystalline_core::skill("crystalline-intelligence")
                .unwrap()
                .content
        )
    );
}

/// **`read_engram` appends one link per resolved reference, and nothing else.**
///
/// Five references go in and two links come out: the duplicate collapses, the
/// fenced example never counted, the absolute URL is somebody else's file and
/// the unresolvable one is left to `evolve_engrams` to flag. The JSON payload
/// stays the first block, so every existing client reads the same first text
/// block it always did.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_engram_links_the_attachments_it_references_and_only_those() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;

    let result = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "shots", "domain": "eng" }),
    )
    .await;

    let first = &result["content"][0];
    assert_eq!(first["type"], json!("text"), "the payload leads: {first}");
    let payload: Value = serde_json::from_str(first["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["permalink"].as_str(), Some("shots"));

    let links = resource_links(&result);
    assert_eq!(
        links.len(),
        2,
        "the two references that resolve, deduped: {:?}",
        result["content"]
    );
    assert_eq!(
        links[0]["uri"].as_str(),
        Some("crystalline://eng/assets/shot.png")
    );
    assert_eq!(
        links[0]["name"].as_str(),
        Some("shot.png"),
        "the name is the filename a person would recognize"
    );
    assert_eq!(links[0]["mimeType"].as_str(), Some("image/png"));
    assert_eq!(links[0]["size"].as_u64(), Some(PNG.len() as u64));

    assert_eq!(
        links[1]["uri"].as_str(),
        Some("crystalline://eng/assets/deep/data.json")
    );
    assert_eq!(
        links[1]["name"].as_str(),
        Some("data.json"),
        "a nested attachment is named by its last segment, not its path"
    );
    assert_eq!(links[1]["mimeType"].as_str(), Some("application/json"));
    assert_eq!(links[1]["size"].as_u64(), Some(JSON.len() as u64));

    // No bytes ever ride in a tool result: that is what the links exist for.
    for block in result["content"].as_array().unwrap() {
        assert!(
            block["blob"].is_null(),
            "a tool result carries links, never base64: {block}"
        );
    }
}

/// An engram referencing nothing gets exactly what it always got: one text
/// block, no links.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engram_without_references_carries_no_links() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;

    let result = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "manifest", "domain": "eng" }),
    )
    .await;
    assert_eq!(
        result["content"].as_array().unwrap().len(),
        1,
        "one text block and nothing appended: {:?}",
        result["content"]
    );
}

/// **The tool list does not move.** The read surface is resources and content
/// blocks; no tool was added, removed or renamed for it, which is what keeps a
/// client's cached listing valid across this change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_list_is_unchanged_by_the_attachment_surface() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;

    let tools = client
        .peer()
        .list_tools(Default::default())
        .await
        .unwrap()
        .tools;
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names.len(), 22, "the one invariant list: {names:?}");
    assert!(
        !names.iter().any(|n| n.contains("attachment")),
        "no attachment tool was added: {names:?}"
    );

    // The one description change is a sentence on `read_engram`, which is what
    // points a model at the links it will now find in the result.
    let read_engram = tools
        .iter()
        .find(|t| t.name.as_ref() == "read_engram")
        .expect("read_engram is listed");
    let description = read_engram.description.as_deref().unwrap_or_default();
    assert!(
        description.contains("resource links") && description.contains("resources/read"),
        "read_engram tells a model what the links are for: {description}"
    );
}

// --- streamable HTTP --------------------------------------------------------

/// One raw HTTP/1.1 POST, read for a bounded window. Mirrors the helper in
/// `tests/mcp_modern_era.rs` rather than sharing it: an integration test binary
/// cannot reach another one's helpers.
async fn post(addr: std::net::SocketAddr, id: u32, method: &str, mut params: Value) -> String {
    let name = params
        .get("name")
        .or_else(|| params.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let body =
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = format!(
        "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Connection: close\r\n\
         MCP-Protocol-Version: {ERA}\r\n\
         Mcp-Method: {method}\r\n"
    );
    if let Some(name) = name {
        head.push_str(&format!("Mcp-Name: {name}\r\n"));
    }
    let request = format!("{head}Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = stream.write_all(request.as_bytes()).await;
    let _ = stream.flush().await;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(2000);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The first JSON-RPC payload in an SSE-framed or plain-JSON response.
fn payload(raw: &str) -> Value {
    let line = raw
        .lines()
        .map(|line| line.strip_prefix("data: ").unwrap_or(line).trim())
        .find(|line| line.starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON-RPC payload in:\n{raw}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("payload is not JSON ({e}):\n{line}"))
}

/// The whole surface again on the other transport: the template, both content
/// shapes, the refusal and the resource links on a tool result. Same server,
/// different rmcp code path, so it is asserted rather than assumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_whole_read_surface_is_served_over_streamable_http_too() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let templates = payload(&post(addr, 1, "resources/templates/list", json!({})).await);
    let listed = templates["result"]["resourceTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("no templates in {templates}"));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0]["uriTemplate"].as_str(),
        Some("crystalline://{domain}/assets/{+path}")
    );
    assert_eq!(listed[0]["name"].as_str(), Some("attachment"));

    let png = payload(
        &post(
            addr,
            2,
            "resources/read",
            json!({ "uri": "crystalline://eng/assets/shot.png" }),
        )
        .await,
    );
    let contents = &png["result"]["contents"][0];
    assert_eq!(contents["mimeType"].as_str(), Some("image/png"));
    assert_eq!(contents["blob"].as_str(), Some(BASE64.encode(PNG).as_str()));

    let data = payload(
        &post(
            addr,
            3,
            "resources/read",
            json!({ "uri": "crystalline://eng/assets/deep/data.json" }),
        )
        .await,
    );
    let contents = &data["result"]["contents"][0];
    assert_eq!(contents["mimeType"].as_str(), Some("application/json"));
    assert_eq!(contents["text"].as_str(), Some(JSON));

    // Percent-encoding is a transport-level fact, so the leg where a client
    // would actually produce it gets its own assertion.
    let encoded = payload(&post(addr, 4, "resources/read", json!({ "uri": UMLAUT_ENCODED })).await);
    let contents = &encoded["result"]["contents"][0];
    assert_eq!(contents["mimeType"].as_str(), Some("image/png"));
    assert_eq!(contents["blob"].as_str(), Some(BASE64.encode(PNG).as_str()));

    let missing = payload(
        &post(
            addr,
            5,
            "resources/read",
            json!({ "uri": "crystalline://eng/assets/gone.png" }),
        )
        .await,
    );
    assert_eq!(
        missing["error"]["code"].as_i64(),
        Some(-32602),
        "an absent attachment is invalid_params here too: {missing}"
    );

    let read = payload(
        &post(
            addr,
            6,
            "tools/call",
            json!({ "name": "read_engram", "arguments": { "identifier": "shots", "domain": "eng" } }),
        )
        .await,
    );
    let links = resource_links(&read["result"]);
    assert_eq!(links.len(), 2, "the two resolved references: {read}");
    assert_eq!(
        links[0]["uri"].as_str(),
        Some("crystalline://eng/assets/shot.png")
    );
    assert_eq!(links[0]["size"].as_u64(), Some(PNG.len() as u64));
    assert_eq!(
        links[1]["uri"].as_str(),
        Some("crystalline://eng/assets/deep/data.json")
    );
}
