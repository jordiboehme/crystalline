//! In-process rmcp duplex tests over the real tool router.
//!
//! A `tokio::io::duplex` pair connects an rmcp client to the `McpServer` in the
//! same process, driving the 12 tools through the actual JSON-RPC path. The
//! engine shares an in-memory store; domains are real temp directories because
//! files are the source of truth.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::sync::Mutex;

struct Harness {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
    root: std::path::PathBuf,
}

impl Harness {
    async fn new(domains: &[&str]) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        for d in domains {
            let dir = root.join(d);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("MANIFEST.md"),
                format!(
                    "---\ntype: manifest\ntitle: {d}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {d}\n\n## Scope\n\n- Everything about {d}\n\n## When to Use\n\n- Route here for {d} questions\n"
                ),
            )
            .unwrap();
            cfg.domains.insert(d.to_string(), DomainEntry { path: dir });
        }
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(Arc::new(Mutex::new(store)), cfg, None, None));
        engine.sync(None).await.unwrap();
        Harness {
            _tmp: tmp,
            engine,
            root,
        }
    }

    async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        // The server handshake blocks until the client sends `initialize`, so the
        // two must run concurrently.
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let client = rmcp::serve_client((), client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }
}

/// Call a tool, returning its JSON body on success.
async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<Value, String> {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match peer.call_tool(params).await {
        Ok(result) => {
            let v = serde_json::to_value(&result).unwrap();
            let text = v
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(serde_json::from_str(text).unwrap_or(Value::String(text.to_string())))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_tools_exposes_all_twelve() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    let names: Vec<String> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    for expected in [
        "write_engram",
        "read_engram",
        "edit_engram",
        "move_engram",
        "delete_engram",
        "search_engrams",
        "build_context",
        "recent_activity",
        "list_domains",
        "browse_domain",
        "validate_engrams",
        "infer_schema",
    ] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
    assert_eq!(names.len(), 12, "exactly 12 tools: {names:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_read_overwrite_and_domain_errors() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    // Happy path write.
    let out = call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "- [decision] we chose alpha #core" }),
    )
    .await
    .unwrap();
    assert_eq!(out["permalink"], json!("alpha"));
    assert_eq!(out["action"], json!("created"));
    assert!(h.root.join("eng/alpha.md").exists());

    // Read it back.
    let read = call(
        peer,
        "read_engram",
        json!({ "identifier": "alpha", "domain": "eng" }),
    )
    .await
    .unwrap();
    assert!(read["content"].as_str().unwrap().contains("chose alpha"));
    assert_eq!(read["title"], json!("Alpha"));

    // Overwrite conflict without the flag.
    let err = call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "dup" }),
    )
    .await
    .unwrap_err();
    assert!(err.to_lowercase().contains("already exists"), "{err}");

    // Overwrite with the flag succeeds.
    let ok = call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Alpha", "content": "replaced", "overwrite": true }),
    )
    .await
    .unwrap();
    assert_eq!(ok["action"], json!("written"));

    // Missing/unknown domain on a write is a tool error listing the registered set.
    let err = call(
        peer,
        "write_engram",
        json!({ "domain": "nope", "title": "X", "content": "y" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("not registered"), "{err}");
    assert!(err.contains("eng"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_operations_and_subsection_regression() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Guide",
            "content": "## API\n\nintro text\n\n### Auth\n\nauth details\n\n## Other\n\ntail",
        }),
    )
    .await
    .unwrap();

    // replace_section on "## API" must keep the ### Auth subsection (regression).
    call(
        peer,
        "edit_engram",
        json!({
            "identifier": "guide",
            "domain": "eng",
            "operation": "replace_section",
            "section": "## API",
            "content": "new api intro",
        }),
    )
    .await
    .unwrap();
    let read = call(
        peer,
        "read_engram",
        json!({ "identifier": "guide", "domain": "eng" }),
    )
    .await
    .unwrap();
    let body = read["content"].as_str().unwrap();
    assert!(body.contains("new api intro"), "section replaced: {body}");
    assert!(body.contains("### Auth"), "subsection kept: {body}");
    assert!(
        body.contains("auth details"),
        "subsection body kept: {body}"
    );

    // Section not found is a tool error.
    let err = call(
        peer,
        "edit_engram",
        json!({
            "identifier": "guide", "domain": "eng", "operation": "replace_section",
            "section": "## Missing", "content": "x",
        }),
    )
    .await
    .unwrap_err();
    assert!(err.to_lowercase().contains("no section"), "{err}");

    // find_replace with a wrong expected count errors instead of editing.
    let err = call(
        peer,
        "edit_engram",
        json!({
            "identifier": "guide", "domain": "eng", "operation": "find_replace",
            "content": "X", "find_text": "api", "expected_replacements": 5,
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("expected 5"), "{err}");

    // append works.
    call(
        peer,
        "edit_engram",
        json!({ "identifier": "guide", "domain": "eng", "operation": "append", "content": "appended line" }),
    )
    .await
    .unwrap();
    let read = call(
        peer,
        "read_engram",
        json!({ "identifier": "guide", "domain": "eng" }),
    )
    .await
    .unwrap();
    assert!(read["content"].as_str().unwrap().contains("appended line"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_filter_only_and_text_fallback() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Widget", "content": "a widget guide", "tags": ["hardware"] }),
    )
    .await
    .unwrap();
    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Gadget", "content": "a gadget guide", "tags": ["software"] }),
    )
    .await
    .unwrap();

    // hybrid requested but no embeddings -> falls back to text mode.
    let out = call(peer, "search_engrams", json!({ "query": "widget" }))
        .await
        .unwrap();
    assert_eq!(out["mode"], json!("text"), "hybrid falls back to text");
    assert!(out["total"].as_u64().unwrap() >= 1);
    let hits = out["hits"].as_array().unwrap();
    assert!(hits.iter().any(|h| h["permalink"] == json!("widget")));
    assert!(
        hits.iter().all(|h| h.get("domain").is_some()),
        "hits labelled with domain"
    );

    // Filter-only search (no query text), by tag.
    let out = call(peer, "search_engrams", json!({ "tags": ["software"] }))
        .await
        .unwrap();
    let hits = out["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["permalink"], json!("gadget"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn move_same_domain_and_cross_domain_link_rewrite() {
    let h = Harness::new(&["eng", "ops"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    // A target and a linker in the same domain.
    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Target", "content": "the target" }),
    )
    .await
    .unwrap();
    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Linker", "content": "see [[Target]] for details" }),
    )
    .await
    .unwrap();

    // Same-domain move (rename).
    let out = call(
        peer,
        "move_engram",
        json!({ "identifier": "target", "domain": "eng", "destination": "archive/target.md" }),
    )
    .await
    .unwrap();
    assert_eq!(out["cross_domain"], json!(false));
    assert!(h.root.join("eng/archive/target.md").exists());

    // Cross-domain move rewrites the inbound bare link to the prefixed form. The
    // same-domain move re-slugged the path-derived permalink to `archive/target`.
    let out = call(
        peer,
        "move_engram",
        json!({
            "identifier": "archive/target",
            "domain": "eng",
            "destination": "target.md",
            "destination_domain": "ops",
        }),
    )
    .await
    .unwrap();
    assert_eq!(out["cross_domain"], json!(true));
    assert_eq!(out["links_rewritten"], json!(1));
    let linker = std::fs::read_to_string(h.root.join("eng/linker.md")).unwrap();
    assert!(
        linker.contains("[[ops:Target]]"),
        "link rewritten: {linker}"
    );
    assert!(h.root.join("ops/target.md").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_context_glob_and_relations() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Root", "content": "- relates_to [[Child]]", "folder": "notes" }),
    )
    .await
    .unwrap();
    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Child", "content": "child body", "folder": "notes" }),
    )
    .await
    .unwrap();

    // Glob anchor over the notes prefix picks up both as seeds.
    let out = call(
        peer,
        "build_context",
        json!({ "anchor": "crystalline://eng/notes/*", "depth": 2 }),
    )
    .await
    .unwrap();
    let nodes = out["nodes"].as_array().unwrap();
    let perms: Vec<&str> = nodes
        .iter()
        .map(|n| n["permalink"].as_str().unwrap())
        .collect();
    assert!(perms.contains(&"notes/root"), "{perms:?}");
    assert!(perms.contains(&"notes/child"), "{perms:?}");
    // The relation edge is present.
    assert!(
        !out["edges"].as_array().unwrap().is_empty(),
        "relation edge present"
    );

    // Bad anchor is a tool error.
    let err = call(peer, "build_context", json!({ "anchor": "not-a-url" }))
        .await
        .unwrap_err();
    assert!(err.contains("crystalline://"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_list_browse_delete() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Fresh", "content": "fresh", "folder": "sub" }),
    )
    .await
    .unwrap();

    // recent_activity finds today's engram.
    let recent = call(peer, "recent_activity", json!({ "timeframe": "7d" }))
        .await
        .unwrap();
    assert!(recent["count"].as_u64().unwrap() >= 1);

    // list_domains with routing bullets from the MANIFEST.
    let domains = call(peer, "list_domains", json!({ "include_routing": true }))
        .await
        .unwrap();
    let d0 = &domains["domains"][0];
    assert_eq!(d0["name"], json!("eng"));
    let bullets = d0["when_to_use"].as_array().unwrap();
    assert!(
        bullets
            .iter()
            .any(|b| b.as_str().unwrap().contains("Route here"))
    );

    // browse_domain lists the sub folder and its engram.
    let browse = call(
        peer,
        "browse_domain",
        json!({ "domain": "eng", "path": "/", "depth": 2 }),
    )
    .await
    .unwrap();
    assert!(
        browse["folders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "sub")
    );

    // delete removes the file and index row.
    let del = call(
        peer,
        "delete_engram",
        json!({ "identifier": "sub/fresh", "domain": "eng" }),
    )
    .await
    .unwrap();
    assert_eq!(del["deleted"], json!(true));
    assert!(!h.root.join("eng/sub/fresh.md").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validate_and_infer_schema() {
    let h = Harness::new(&["eng"]).await;

    // A schema engram requiring a `decision` observation on `note` engrams. It is
    // written to disk then indexed via a sync, mirroring a curated schema file.
    let schema_md = "---\ntype: schema\ntitle: Note Schema\npermalink: note-schema\ntags:\n  - schema\nstatus: current\nrecorded_at: 2026-01-01\nentity: note\nversion: 1\nschema:\n  decision: string\nsettings:\n  validation: warn\n---\n\n# Note Schema\n";
    std::fs::write(h.root.join("eng/note-schema.md"), schema_md).unwrap();
    h.engine.sync(None).await.unwrap();

    let (client, _server) = h.connect().await;
    let peer = client.peer();

    // A conforming and a non-conforming note.
    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Good", "type": "note", "content": "- [decision] do it" }),
    )
    .await
    .unwrap();
    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Bad", "type": "note", "content": "no observations here" }),
    )
    .await
    .unwrap();

    let out = call(
        peer,
        "validate_engrams",
        json!({ "domain": "eng", "type": "note" }),
    )
    .await
    .unwrap();
    assert!(out["schemas"].as_u64().unwrap() >= 1);
    let issues = out["issues"].as_array().unwrap();
    assert!(
        issues.iter().any(|i| i["permalink"] == json!("bad")),
        "the non-conforming note is flagged: {issues:?}"
    );

    let inferred = call(
        peer,
        "infer_schema",
        json!({ "domain": "eng", "type": "note" }),
    )
    .await
    .unwrap();
    assert_eq!(inferred["type"], json!("note"));
    assert!(inferred["count"].as_u64().unwrap() >= 2);
}
