//! In-process rmcp duplex tests over the real tool router.
//!
//! A `tokio::io::duplex` pair connects an rmcp client to the `McpServer` in the
//! same process, driving the tools through the actual JSON-RPC path. The
//! engine shares an in-memory store; domains are real temp directories because
//! files are the source of truth.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::path::Path;

struct Harness {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
    root: std::path::PathBuf,
}

impl Harness {
    async fn new(domains: &[&str]) -> Harness {
        Harness::build(domains, false).await
    }

    /// A harness whose engine serves the content API read-only.
    async fn new_read_only(domains: &[&str]) -> Harness {
        Harness::build(domains, true).await
    }

    async fn build(domains: &[&str], read_only: bool) -> Harness {
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
            cfg.domains.insert(d.to_string(), DomainEntry::file(dir));
        }
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, None).with_read_only(read_only),
        );
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
async fn list_tools_exposes_the_core_tools_plus_configure_and_add_domain() {
    // With GitHub collaboration off (the default), `configure` is the only
    // one of the five collaboration tools that is ever visible: see
    // `crate::mcp::hidden_collab_tool` and its dedicated gating matrix tests.
    // `add_domain` is not collaboration-gated: it creates domains of every kind
    // and is visible whenever the instance is writable.
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
        "configure",
        "add_domain",
    ] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
    for hidden in [
        "share_changes",
        "update_domain",
        "origin_status",
        "resolve_conflict",
    ] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} must be hidden while github.enabled is off: {names:?}"
        );
    }
    assert_eq!(names.len(), 14, "exactly 14 tools: {names:?}");
}

/// The `configure` tool's `set` and `unset` inputs must advertise the plain
/// `object`/`array` JSON Schema `type` rather than a `["object", "null"]` or
/// `["array", "null"]` union: some MCP clients (Claude Desktop) do not
/// recognize the union form and stringify object/array values on the wire,
/// which then fails to deserialize with `expected a map`/`expected a sequence`.
/// Regression guard for that class of client-side stringification.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_tool_schema_advertises_plain_object_and_array_types() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    let configure = tools
        .tools
        .iter()
        .find(|t| t.name == "configure")
        .expect("configure tool present");
    let schema = serde_json::to_value(&configure.input_schema).unwrap();
    let set_type = &schema["properties"]["set"]["type"];
    assert_eq!(
        set_type,
        &json!("object"),
        "configure.set must advertise a plain object type, got {set_type}"
    );
    let unset_type = &schema["properties"]["unset"]["type"];
    assert_eq!(
        unset_type,
        &json!("array"),
        "configure.unset must advertise a plain array type, got {unset_type}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_hides_the_write_gated_tools() {
    let h = Harness::new_read_only(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    let names: Vec<String> = tools.tools.iter().map(|t| t.name.to_string()).collect();

    // The five write-gated tools (four content-mutating plus add_domain, which
    // creates domains) are absent from the surface.
    for hidden in [
        "write_engram",
        "edit_engram",
        "move_engram",
        "delete_engram",
        "add_domain",
    ] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} must be hidden in read-only mode: {names:?}"
        );
    }
    // The eight read tools remain.
    for expected in [
        "read_engram",
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
    // Read-only and GitHub collaboration off together hide all five
    // collaboration tools too (the full gating matrix lives in
    // tests/mcp_collab.rs).
    for hidden in [
        "configure",
        "share_changes",
        "update_domain",
        "origin_status",
        "resolve_conflict",
    ] {
        assert!(
            !names.contains(&hidden.to_string()),
            "{hidden} must be hidden read-only: {names:?}"
        );
    }
    assert_eq!(names.len(), 8, "exactly 8 tools in read-only: {names:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_call_by_name_returns_the_read_only_error() {
    let h = Harness::new_read_only(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    // Calling a hidden tool by name is dispatched to the engine guard, which
    // returns the read-only error rather than a bare "tool not found".
    let err = call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Nope", "content": "no" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("read-only"), "read-only error expected: {err}");
    // Nothing was written.
    assert!(!h.root.join("eng/nope.md").exists());
}

/// The engine guard is defense in depth: it refuses the four mutating methods
/// regardless of the MCP layer, so the embedded CLI dispatch is covered too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_engine_guard_refuses_mutations() {
    use crystalline_service::EngineError;
    use crystalline_service::params::{DeleteParams, WriteParams};

    let h = Harness::new_read_only(&["eng"]).await;

    let write: WriteParams =
        serde_json::from_value(json!({ "domain": "eng", "title": "Blocked", "content": "body" }))
            .unwrap();
    assert!(
        matches!(
            h.engine.write_engram(&write).await,
            Err(EngineError::ReadOnly)
        ),
        "write_engram must refuse on a read-only engine"
    );

    let delete: DeleteParams =
        serde_json::from_value(json!({ "identifier": "anything", "domain": "eng" })).unwrap();
    assert!(
        matches!(
            h.engine.delete_engram(&delete).await,
            Err(EngineError::ReadOnly)
        ),
        "delete_engram must refuse on a read-only engine"
    );
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

/// The identifier grammar is strict: an identifier without the
/// crystalline:// scheme is domain-relative, always, so a domain-prefixed
/// composite never resolves - and when a domain is passed alongside, the
/// error teaches the bare form back so an agent recovers in one step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_prefixed_identifier_never_resolves_and_the_error_teaches_the_fix() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Guide", "content": "the guide body text" }),
    )
    .await
    .unwrap();

    // edit_engram with the composite identifier plus domain: the exact
    // failing call agents keep producing; the hint names the bare form.
    let err = call(
        peer,
        "edit_engram",
        json!({
            "identifier": "eng/guide", "domain": "eng",
            "operation": "append", "content": "x",
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("retry with 'guide'"), "{err}");

    // read_engram with both earns the same hint.
    let err = call(
        peer,
        "read_engram",
        json!({ "identifier": "eng/guide", "domain": "eng" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("retry with 'guide'"), "{err}");

    // A plain miss keeps the plain message, no hint.
    let err = call(
        peer,
        "read_engram",
        json!({ "identifier": "nope", "domain": "eng" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("no engram"), "{err}");
    assert!(!err.contains("retry with"), "{err}");

    // Without a domain the composite still never resolves: scheme-less is
    // domain-relative, never a domain prefix.
    let err = call(peer, "read_engram", json!({ "identifier": "eng/guide" }))
        .await
        .unwrap_err();
    assert!(err.contains("no engram matches"), "{err}");

    // The one absolute, cross-domain form is the crystalline:// URL.
    let read = call(
        peer,
        "read_engram",
        json!({ "identifier": "crystalline://eng/guide" }),
    )
    .await
    .unwrap();
    assert_eq!(read["title"], json!("Guide"));
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

/// Models routinely double-encode nested tool arguments: a `metadata`
/// object arriving as a JSON string is accepted by parsing it first, so
/// temporal bounds set that way land in the frontmatter instead of
/// erroring. Anything that is not an object, before or after decoding,
/// still fails with the plain must-be-an-object error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_engram_accepts_metadata_as_a_json_encoded_string() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Bounded policy",
            "content": "A bounded rule.\n\n- [fact] bounded #eng\n- [fact] expires soon #eng",
            "tags": ["eng"],
            "metadata": "{\"valid_to\": \"2026-09-30\"}",
        }),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(h.root.join("eng/bounded-policy.md")).unwrap();
    assert!(text.contains("valid_to"), "valid_to missing: {text}");
    assert!(text.contains("2026-09-30"), "bound value missing: {text}");

    let err = call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Nope",
            "content": "x",
            "metadata": "not json at all",
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("must be an object"), "unexpected error: {err}");
}

/// The write path enforces the temporal contract: a date field carrying a
/// time-of-day component is rejected with the day-granular message and no file
/// is created. write_engram and the CLI write share this engine call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_engram_rejects_a_timestamp_in_a_date_field() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let err = call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Timestamped bound",
            "content": "A rule.",
            "metadata": { "valid_to": "2026-07-15T10:30:00Z" },
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("must be a plain ISO date (YYYY-MM-DD)"),
        "unexpected error: {err}"
    );
    assert!(
        !h.root.join("eng/timestamped-bound.md").exists(),
        "the engram file must not be created when the write is rejected"
    );
}

/// A sentinel far-future `valid_to` is dropped on write: open-ended validity is
/// expressed by absence, not by a distant date. The write succeeds and the
/// field never lands in the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_engram_drops_a_sentinel_valid_to() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Open ended",
            "content": "Valid forever.",
            "metadata": { "valid_to": "9999-12-30" },
        }),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(h.root.join("eng/open-ended.md")).unwrap();
    assert!(
        !text.contains("valid_to"),
        "the sentinel valid_to must be dropped: {text}"
    );
}

/// A plain ISO `valid_from` is promoted into the typed frontmatter field, so
/// read_engram returns it as a date rather than an unparsed extra value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_engram_promotes_a_plain_valid_from() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({
            "domain": "eng",
            "title": "Bounded start",
            "content": "Valid from a date.",
            "metadata": { "valid_from": "2026-01-01" },
        }),
    )
    .await
    .unwrap();
    let out = call(
        peer,
        "read_engram",
        json!({ "identifier": "bounded-start", "domain": "eng" }),
    )
    .await
    .unwrap();
    assert_eq!(
        out["frontmatter"]["valid_from"],
        json!("2026-01-01"),
        "valid_from must be a typed date: {out}"
    );
}

/// The edit path enforces the same contract: a find_replace that injects a
/// timestamped `valid_to` into the frontmatter is rejected and the file on disk
/// is left byte-for-byte unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_engram_rejects_an_injected_timestamp_bound() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Editable", "content": "A rule." }),
    )
    .await
    .unwrap();
    let path = h.root.join("eng/editable.md");
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(
        before.contains("status: current"),
        "the edit anchor is missing: {before}"
    );

    let err = call(
        peer,
        "edit_engram",
        json!({
            "domain": "eng",
            "identifier": "editable",
            "operation": "find_replace",
            "find_text": "status: current",
            "content": "status: current\nvalid_to: 2026-07-15T10:30:00Z",
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("must be a plain ISO date (YYYY-MM-DD)"),
        "unexpected error: {err}"
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        before, after,
        "the file must be unchanged after a rejected edit"
    );
}

/// An edit that injects a sentinel far-future `valid_to` succeeds, but the
/// bound is surgically dropped so absence keeps expressing open-ended validity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_engram_drops_an_injected_sentinel_bound() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Sentinel edit", "content": "A rule." }),
    )
    .await
    .unwrap();
    call(
        peer,
        "edit_engram",
        json!({
            "domain": "eng",
            "identifier": "sentinel-edit",
            "operation": "find_replace",
            "find_text": "status: current",
            "content": "status: current\nvalid_to: 9999-12-30",
        }),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(h.root.join("eng/sentinel-edit.md")).unwrap();
    assert!(
        !text.contains("valid_to"),
        "the sentinel valid_to must be dropped: {text}"
    );
    assert!(
        !text.contains("9999-12-30"),
        "the sentinel value must be gone: {text}"
    );
}

/// recorded_at is the one temporal field a null cannot drop: it is required
/// (T001), so an edit that blanks it is rejected and the file on disk is left
/// byte-for-byte unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_engram_rejects_a_nulled_recorded_at() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    call(
        peer,
        "write_engram",
        json!({ "domain": "eng", "title": "Recorded", "content": "A rule." }),
    )
    .await
    .unwrap();
    let path = h.root.join("eng/recorded.md");
    let before = std::fs::read_to_string(&path).unwrap();
    let recorded_line = before
        .lines()
        .find(|l| l.starts_with("recorded_at: "))
        .unwrap_or_else(|| panic!("no recorded_at line: {before}"));

    let err = call(
        peer,
        "edit_engram",
        json!({
            "domain": "eng",
            "identifier": "recorded",
            "operation": "find_replace",
            "find_text": recorded_line,
            "content": "recorded_at:",
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("recorded_at must be a plain ISO date"),
        "unexpected error: {err}"
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        before, after,
        "the file must be unchanged after a rejected edit"
    );
}

/// validate_engrams runs the temporal checks: a date field written straight to
/// disk with a time-of-day component is reported as a T003 issue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validate_engrams_flags_a_malformed_date_as_t003() {
    let h = Harness::new(&["eng"]).await;
    let md = "---\ntype: note\ntitle: Bad Date\npermalink: bad-date\ntags:\n  - eng\nstatus: current\nrecorded_at: 2026-01-01\nvalid_from: 2026-07-15T10:30:00Z\n---\n\n# Bad Date\n";
    std::fs::write(h.root.join("eng/bad-date.md"), md).unwrap();
    h.engine.sync(None).await.unwrap();

    let (client, _server) = h.connect().await;
    let out = call(
        client.peer(),
        "validate_engrams",
        json!({ "domain": "eng" }),
    )
    .await
    .unwrap();
    let issues = out["issues"].as_array().unwrap();
    assert!(
        issues.iter().any(|i| i["kind"] == json!("T003")),
        "a T003 issue must be present: {issues:?}"
    );
}

/// The locked MCP tool annotation table: one row per tool, each a tuple of
/// (name, title, read_only_hint, destructive_hint, idempotent_hint,
/// open_world_hint). A hint of `None` means the attribute is deliberately
/// absent from the wire (destructive and idempotent are omitted on the read
/// tools, where they are meaningless per the MCP spec). This is the source of
/// truth the tests below lock the router against.
type AnnotationRow = (
    &'static str,
    &'static str,
    Option<bool>,
    Option<bool>,
    Option<bool>,
    Option<bool>,
);

const EXPECTED_ANNOTATIONS: [AnnotationRow; 18] = [
    (
        "write_engram",
        "Capture engram",
        Some(false),
        Some(false),
        Some(false),
        Some(false),
    ),
    (
        "read_engram",
        "Read engram",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "edit_engram",
        "Edit engram",
        Some(false),
        Some(true),
        Some(false),
        Some(false),
    ),
    (
        "move_engram",
        "Move engram",
        Some(false),
        Some(true),
        Some(false),
        Some(false),
    ),
    (
        "delete_engram",
        "Delete engram",
        Some(false),
        Some(true),
        Some(true),
        Some(false),
    ),
    (
        "search_engrams",
        "Search engrams",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "build_context",
        "Build context",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "recent_activity",
        "Recent activity",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "list_domains",
        "List domains",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "browse_domain",
        "Browse domain",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "validate_engrams",
        "Validate engrams",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "infer_schema",
        "Infer schema",
        Some(true),
        None,
        None,
        Some(false),
    ),
    (
        "configure",
        "Configure Crystalline",
        Some(false),
        Some(true),
        Some(false),
        Some(true),
    ),
    (
        "add_domain",
        "Add domain",
        Some(false),
        Some(false),
        Some(false),
        Some(true),
    ),
    (
        "share_changes",
        "Share changes",
        Some(false),
        Some(false),
        Some(false),
        Some(true),
    ),
    (
        "update_domain",
        "Update domain",
        Some(false),
        Some(false),
        Some(true),
        Some(true),
    ),
    (
        "origin_status",
        "Origin status",
        Some(true),
        None,
        None,
        Some(true),
    ),
    (
        "resolve_conflict",
        "Resolve conflict",
        Some(false),
        Some(true),
        Some(false),
        Some(false),
    ),
];

/// A GitHub-enabled server with no domains, built solely to inspect the tool
/// surface and its annotations. GitHub on plus read-write makes all 18 tools
/// visible through `get_tool`; read-only narrows it to the read tools plus
/// `update_domain` and `origin_status`.
async fn annotation_server(read_only: bool) -> McpServer {
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, None).with_read_only(read_only),
    );
    McpServer::new(engine)
}

/// Every tool advertises exactly the title and the four annotation hints from
/// the locked table, and never the annotation-level title (only the top-level
/// `Tool.title`). GitHub is enabled and the engine read-write so all 18 tools
/// are visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_annotations_match_the_locked_table() {
    use rmcp::ServerHandler;

    let server = annotation_server(false).await;
    for (name, title, read_only, destructive, idempotent, open_world) in EXPECTED_ANNOTATIONS {
        let tool = server
            .get_tool(name)
            .unwrap_or_else(|| panic!("tool {name} must be visible"));
        assert_eq!(tool.title.as_deref(), Some(title), "{name} title");
        let ann = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{name} must carry annotations"));
        assert_eq!(ann.read_only_hint, read_only, "{name} read_only_hint");
        assert_eq!(ann.destructive_hint, destructive, "{name} destructive_hint");
        assert_eq!(ann.idempotent_hint, idempotent, "{name} idempotent_hint");
        assert_eq!(ann.open_world_hint, open_world, "{name} open_world_hint");
        assert_eq!(ann.title, None, "{name} annotations.title must stay unset");
    }
}

/// Two invariants tie the advisory hints back to the runtime gating. First,
/// every write-gated tool advertises read_only_hint == Some(false). Second,
/// every tool still visible in read-only mode is a read tool (read_only_hint ==
/// Some(true)) or exactly `update_domain`, the documented derived-truth
/// exemption that a pull shares with sync.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn annotation_hints_line_up_with_the_gating() {
    use rmcp::ServerHandler;

    // The eight write-gated tools (the five WRITE_TOOLS plus the three
    // collaboration tools hidden in read-only mode) must each disclaim
    // read-only.
    let rw = annotation_server(false).await;
    for name in [
        "write_engram",
        "edit_engram",
        "move_engram",
        "delete_engram",
        "add_domain",
        "configure",
        "share_changes",
        "resolve_conflict",
    ] {
        let tool = rw
            .get_tool(name)
            .unwrap_or_else(|| panic!("tool {name} must be visible"));
        let read_only_hint = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
        assert_eq!(
            read_only_hint,
            Some(false),
            "{name} is write-gated so it must not advertise read-only"
        );
    }

    // Everything still visible on a read-only server is a read tool or exactly
    // update_domain.
    let ro = annotation_server(true).await;
    for (name, ..) in EXPECTED_ANNOTATIONS {
        let Some(tool) = ro.get_tool(name) else {
            continue;
        };
        let read_only_hint = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
        assert!(
            read_only_hint == Some(true) || name == "update_domain",
            "{name} is visible read-only but is neither a read tool nor update_domain"
        );
    }
}

// --- provision: declaration gating -------------------------------------------

/// Overwrites `domain`'s MANIFEST.md under `root` (a `Harness`'s domain root)
/// so it declares a `## Provisioning` section - `Harness::build`'s own
/// template never does, so the gating tests below need this to flip a domain
/// from undeclared to declared.
fn declare_provisioning(root: &std::path::Path, domain: &str) {
    std::fs::write(
        root.join(domain).join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {domain}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n\
             # {domain}\n\n\
             ## Scope\n\n- Everything about {domain}\n\n\
             ## When to Use\n\n- Route here for {domain} questions\n\n\
             ## Provisioning\n\n- skills: skills\n"
        ),
    )
    .unwrap();
}

/// A domain with no `Provisioning` section never surfaces the tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_tool_hidden_when_no_domain_declares() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    assert!(
        !tools.tools.iter().any(|t| t.name == "provision"),
        "provision must be hidden when no domain declares a Provisioning section: {:?}",
        tools.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}

/// Once a domain's MANIFEST declares a `Provisioning` section, the tool shows
/// up in both `list_tools` and `get_tool`, the two enforcement points.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_tool_visible_once_a_domain_declares() {
    use rmcp::ServerHandler;

    let h = Harness::new(&["harbor"]).await;
    declare_provisioning(&h.root, "harbor");

    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    assert!(
        tools.tools.iter().any(|t| t.name == "provision"),
        "provision must be visible once harbor declares: {:?}",
        tools.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    let server = McpServer::new(h.engine.clone());
    let tool = server
        .get_tool("provision")
        .expect("get_tool must agree with list_tools");

    // The annotation row `EXPECTED_ANNOTATIONS` cannot carry (its fixture has
    // no declaring domain, so the tool is hidden there): destructive because
    // deny removes installed artifacts, idempotent because re-running any
    // action reconciles to the same state, closed-world because everything
    // happens on this machine.
    assert_eq!(tool.title.as_deref(), Some("Provision harness artifacts"));
    let ann = tool.annotations.as_ref().expect("annotations present");
    assert_eq!(ann.read_only_hint, Some(false));
    assert_eq!(ann.destructive_hint, Some(true));
    assert_eq!(ann.idempotent_hint, Some(true));
    assert_eq!(ann.open_world_hint, Some(false));
}

/// A declared domain still hides `provision` on a read-only instance, at both
/// enforcement points.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_tool_hidden_in_read_only() {
    use rmcp::ServerHandler;

    let h = Harness::new_read_only(&["harbor"]).await;
    declare_provisioning(&h.root, "harbor");

    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    assert!(
        !tools.tools.iter().any(|t| t.name == "provision"),
        "provision must be hidden read-only even though harbor declares: {:?}",
        tools.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    let server = McpServer::new(h.engine.clone());
    assert!(
        server.get_tool("provision").is_none(),
        "get_tool must agree with list_tools"
    );
}

// --- provision: HOME/XDG_STATE_HOME redirection (unix only) -----------------
//
// `Engine::provision` always resolves `install_receipt_path`/`receipt_path`
// (and a harness's artifact base) from `HOME`/`XDG_STATE_HOME`, even for a
// bare `status` call - see `crates/service/tests/provision.rs`'s own note,
// the engine-level sibling of the tests below. Every test that calls the
// `provision` tool redirects them to a scratch directory first, restoring
// the surrounding environment on drop.

/// Serializes every HOME/XDG_STATE_HOME-mutating test in this binary. A
/// tokio mutex, not `std::sync::Mutex`: the guard below is held across
/// `.await` points in the async tests, which clippy's `await_holding_lock`
/// flags for a std lock (the same reason `tests/provision.rs` uses one).
#[cfg(unix)]
static PROVISION_ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Points `HOME`/`XDG_STATE_HOME` at scratch directories for the duration of
/// one test, restoring whatever the surrounding environment had on drop.
#[cfg(unix)]
struct ProvisionScratchEnv {
    previous: (Option<OsString>, Option<OsString>),
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl ProvisionScratchEnv {
    async fn new(home: &Path, xdg_state_home: &Path) -> ProvisionScratchEnv {
        let guard = PROVISION_ENV_LOCK.lock().await;
        let previous = (std::env::var_os("HOME"), std::env::var_os("XDG_STATE_HOME"));
        // SAFETY: guarded by PROVISION_ENV_LOCK, restored on drop.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("XDG_STATE_HOME", xdg_state_home);
        }
        ProvisionScratchEnv {
            previous,
            _guard: guard,
        }
    }
}

#[cfg(unix)]
impl Drop for ProvisionScratchEnv {
    fn drop(&mut self) {
        match &self.previous.0 {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match &self.previous.1 {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
    }
}

/// A harbor-shaped MANIFEST declaring one skill (no mcps - this suite never
/// wants a real harness CLI on `PATH`), mirroring `tests/provision.rs`'s own
/// `write_harbor` fixture.
#[cfg(unix)]
fn write_provision_harbor(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n\
         # harbor\n\n\
         ## Scope\n\n- Coastal navigation knowledge\n\n\
         ## When to Use\n\n- When docking\n\n\
         ## Provisioning\n\n- skills: skills\n",
    )
    .unwrap();
    let skill = dir.join("skills/tide-tables/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(
        skill,
        "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n",
    )
    .unwrap();
}

/// Marks claude-code onboarded in the install receipt this test's isolated
/// `XDG_STATE_HOME` resolves to, so `Engine::provision` finds a harness to
/// reconcile into.
#[cfg(unix)]
fn write_provision_install_receipt(xdg_state_home: &Path) {
    let path = xdg_state_home.join("crystalline").join("installs.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "format": 1,
            "installs": [
                {
                    "harness": "claude-code",
                    "scope": "user",
                    "version": "0.0.0",
                    "parts": { "mcp": true, "hooks": true, "skills": true },
                    "skills": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

/// `allow` records the decision on the config file this engine owns and
/// reconciles a skill into the isolated `HOME`, then `status` reports the
/// decision back.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_allow_then_status_flow() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let _env = ProvisionScratchEnv::new(&home, &xdg_state_home).await;

    let harbor_dir = work.path().join("kb-harbor");
    write_provision_harbor(&harbor_dir);
    write_provision_install_receipt(&xdg_state_home);

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("harbor".to_string(), DomainEntry::file(harbor_dir));
    let config_path = work.path().join("config.yaml");
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path.clone()),
    ));
    assert!(engine.provisioning_declared());

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_engine = engine.clone();
    let server_task =
        tokio::spawn(
            async move { rmcp::serve_server(McpServer::new(server_engine), server_io).await },
        );
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    let peer = client.peer();

    let allow = call(
        peer,
        "provision",
        json!({"action": "allow", "domain": "harbor"}),
    )
    .await
    .unwrap();
    let harnesses = allow["harnesses"].as_array().unwrap();
    assert_eq!(harnesses.len(), 1, "{allow}");
    assert_eq!(harnesses[0]["harness"], "claude-code");
    let actions = harnesses[0]["actions"].as_array().unwrap();
    assert!(
        actions.iter().any(|a| a["status"] == "installed"),
        "{allow}"
    );

    // The decision landed on the config file this engine owns, not just in
    // memory.
    let saved: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    assert_eq!(saved.domains["harbor"].provision, Some(true));

    // The files actually landed under the isolated HOME.
    assert!(home.join(".claude/skills/tide-tables/SKILL.md").exists());

    let status = call(peer, "provision", json!({"action": "status"}))
        .await
        .unwrap();
    let domains = status["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{status}");
    assert_eq!(domains[0]["domain"], "harbor");
    assert_eq!(domains[0]["decision"], "allowed");
    assert!(
        status["pending"].as_array().unwrap().is_empty(),
        "harbor is decided, not pending: {status}"
    );

    drop(client);
    drop(server);
}

/// Calling `provision` by name while it is hidden still reaches the engine:
/// `status` with no declaring domain answers for real (an empty report), not
/// "tool not found", and `allow` on a read-only instance gets the read-only
/// refusal rather than a bare not-found error.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provision_call_by_name_while_hidden_reaches_engine() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let _env = ProvisionScratchEnv::new(&home, &xdg_state_home).await;

    // No declaring domain at all: the tool is absent from list_tools, but a
    // direct call by name still reaches the engine.
    let h = Harness::new(&[]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let tools = peer.list_tools(Default::default()).await.unwrap();
    assert!(
        !tools.tools.iter().any(|t| t.name == "provision"),
        "provision must be hidden with no declaring domain"
    );

    let status = call(peer, "provision", json!({"action": "status"}))
        .await
        .unwrap();
    assert!(status["domains"].as_array().unwrap().is_empty(), "{status}");
    assert!(
        status["harnesses"].as_array().unwrap().is_empty(),
        "{status}"
    );
    assert!(status["pending"].as_array().unwrap().is_empty(), "{status}");

    // Read-only + allow by name: the read-only refusal, not "tool not found".
    let ro = Harness::new_read_only(&["eng"]).await;
    let (ro_client, _ro_server) = ro.connect().await;
    let ro_peer = ro_client.peer();
    let err = call(
        ro_peer,
        "provision",
        json!({"action": "allow", "domain": "eng"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("read-only"), "{err}");
}

// --- provision: tools/list_changed on a declaration flip ---------------------

/// A client handler that records whether it ever received
/// `notifications/tools/list_changed`, mirroring
/// `tests/mcp_collab.rs`'s `NotifyClient` for the `configure` flip.
#[derive(Clone, Default)]
struct ProvisionNotifyClient {
    got_list_changed: Arc<tokio::sync::Notify>,
}

impl rmcp::ClientHandler for ProvisionNotifyClient {
    fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        let notify = self.got_list_changed.clone();
        async move {
            notify.notify_one();
        }
    }
}

/// `add_domain` adopting a folder whose MANIFEST already declares a
/// `Provisioning` section flips `provisioning_declared` from false to true,
/// which must push a `tools/list_changed` notification the same way
/// `configure` does for `github.enabled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_domain_flip_notifies_tool_list_changed() {
    let tmp = tempfile::tempdir().unwrap();
    // A real tempdir config path, never the developer's actual global config:
    // `add_domain` persists a freshly registered domain through it.
    let config_path = tmp.path().join("config.yaml");
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        GlobalConfig::default(),
        None,
        Some(config_path),
    ));
    assert!(!engine.provisioning_declared());

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_engine = engine.clone();
    let server_task =
        tokio::spawn(
            async move { rmcp::serve_server(McpServer::new(server_engine), server_io).await },
        );
    let handler = ProvisionNotifyClient::default();
    let client = rmcp::serve_client(handler.clone(), client_io)
        .await
        .unwrap();
    let _server = server_task.await.unwrap().unwrap();
    let peer = client.peer();

    // A folder whose MANIFEST already declares a Provisioning section: adding
    // it as a domain adopts it in place and flips provisioning_declared.
    let dir = tmp.path().join("harbor");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n\
         # harbor\n\n\
         ## Scope\n\n- Coastal navigation knowledge\n\n\
         ## When to Use\n\n- When docking\n\n\
         ## Provisioning\n\n- skills: skills\n",
    )
    .unwrap();

    call(
        peer,
        "add_domain",
        json!({"domain": "harbor", "folder": dir.to_string_lossy()}),
    )
    .await
    .unwrap();

    assert!(engine.provisioning_declared());

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        handler.got_list_changed.notified(),
    )
    .await
    .expect(
        "expected a tools/list_changed notification after add_domain flipped provisioning_declared",
    );
}

// --- tool schema sanitizer: advertised-shape sweep ---------------------------

/// The JSON Schema `format` values these tools may advertise on purpose,
/// duplicated from `crate::tool_schema`'s own allowlist rather than imported
/// from it (the module is private and this check is meant to stand on its
/// own): a bug shared between the two lists would otherwise hide from both.
const CONSERVATIVE_FORMAT_ALLOWLIST: &[&str] = &[
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

/// A naive whole-tree sweep over an advertised schema: it walks every object
/// and array value recursively rather than the fixed set of positions
/// `crate::tool_schema::sanitize_schema` recurses into, so it is a check on
/// what actually crossed the wire rather than a restatement of the
/// sanitizer's own recursion list. Asserts, everywhere in the tree: (a) an
/// array under a `"type"` key never contains the string `"null"`; (b) a
/// string value under a `"format"` key is in `CONSERVATIVE_FORMAT_ALLOWLIST`;
/// (c) whenever an object carries a `"properties"` key, every property value
/// has at least one of type/$ref/enum/const/anyOf/oneOf/allOf/not. `context`
/// is included in every assertion message so a failure names the tool and
/// server it came from.
fn assert_conservative(schema: &Value, context: &str) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Array(members)) = map.get("type") {
                assert!(
                    !members.iter().any(|v| v.as_str() == Some("null")),
                    "{context}: a type array must never contain null: {members:?}"
                );
            }
            if let Some(format) = map.get("format") {
                assert!(
                    format
                        .as_str()
                        .is_some_and(|f| CONSERVATIVE_FORMAT_ALLOWLIST.contains(&f)),
                    "{context}: format {format:?} is not in the conservative allowlist"
                );
            }
            if let Some(Value::Object(properties)) = map.get("properties") {
                for (property_name, property) in properties {
                    let Value::Object(property) = property else {
                        continue;
                    };
                    let has_type_bearing_key = [
                        "type", "$ref", "enum", "const", "anyOf", "oneOf", "allOf", "not",
                    ]
                    .iter()
                    .any(|k| property.contains_key(*k));
                    assert!(
                        has_type_bearing_key,
                        "{context}: property {property_name} has neither a type nor a combinator: {property:?}"
                    );
                }
            }
            for value in map.values() {
                assert_conservative(value, context);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_conservative(item, context);
            }
        }
        _ => {}
    }
}

/// Every one of the 18 tools in `EXPECTED_ANNOTATIONS` advertises an input
/// schema that passes the naive conservative-shape sweep, both on the
/// read-write server where all 18 are visible and on the read-only one where
/// only a subset resolves through `get_tool`. Also locks down the two
/// type-less `serde_json::Value` params in this codebase to their documented
/// object shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_advertised_tool_schema_is_client_conservative() {
    use rmcp::ServerHandler;

    let rw = annotation_server(false).await;
    for (name, ..) in EXPECTED_ANNOTATIONS {
        let tool = rw
            .get_tool(name)
            .unwrap_or_else(|| panic!("tool {name} must be visible on the read-write server"));
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_conservative(&schema, &format!("{name} input schema (read-write)"));
    }

    // Read-only narrows visibility (see `annotation_hints_line_up_with_the_gating`),
    // so only the tools still resolving through `get_tool` are swept here.
    let ro = annotation_server(true).await;
    for (name, ..) in EXPECTED_ANNOTATIONS {
        let Some(tool) = ro.get_tool(name) else {
            continue;
        };
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_conservative(&schema, &format!("{name} input schema (read-only)"));
    }

    let write_engram = rw.get_tool("write_engram").expect("write_engram visible");
    let write_schema = serde_json::to_value(&write_engram.input_schema).unwrap();
    assert_eq!(
        write_schema["properties"]["metadata"]["type"],
        json!("object"),
        "write_engram.metadata must advertise a plain object type"
    );

    let search_engrams = rw
        .get_tool("search_engrams")
        .expect("search_engrams visible");
    let search_schema = serde_json::to_value(&search_engrams.input_schema).unwrap();
    assert_eq!(
        search_schema["properties"]["metadata_filters"]["type"],
        json!("object"),
        "search_engrams.metadata_filters must advertise a plain object type"
    );
}

/// The same sweep, but driven through the real `tools/list` JSON-RPC wire
/// path (the duplex `Harness` from `configure_tool_schema_advertises_plain_object_and_array_types`)
/// rather than the in-process `get_tool` handler, so a regression in
/// serialization or in the rmcp router itself would also be caught. Also
/// spot-checks `search_engrams.limit` and `.min_similarity`, the `Option<usize>`
/// and `Option<f32>` fields whose schemars-generated `format` (`uint`, `float`)
/// must be gone while `limit`'s `minimum` keyword survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_list_tools_carries_sanitized_schemas() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let tools = client.peer().list_tools(Default::default()).await.unwrap();

    for tool in &tools.tools {
        let schema = serde_json::to_value(&tool.input_schema).unwrap();
        assert_conservative(
            &schema,
            &format!("{} input schema over the wire", tool.name),
        );
    }

    let search_engrams = tools
        .tools
        .iter()
        .find(|t| t.name == "search_engrams")
        .expect("search_engrams tool present");
    let schema = serde_json::to_value(&search_engrams.input_schema).unwrap();
    assert_eq!(
        schema["properties"]["limit"]["type"],
        json!("integer"),
        "search_engrams.limit must advertise a bare integer type"
    );
    assert!(
        schema["properties"]["limit"].get("minimum").is_some(),
        "search_engrams.limit must keep its minimum keyword"
    );
    assert!(
        schema["properties"]["limit"].get("format").is_none(),
        "search_engrams.limit must not advertise a format"
    );
    assert_eq!(
        schema["properties"]["min_similarity"]["type"],
        json!("number"),
        "search_engrams.min_similarity must advertise a bare number type"
    );
    assert!(
        schema["properties"]["min_similarity"]
            .get("format")
            .is_none(),
        "search_engrams.min_similarity must not advertise a format"
    );
}
