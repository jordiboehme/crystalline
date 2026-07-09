//! In-process rmcp duplex tests over the real tool router.
//!
//! A `tokio::io::duplex` pair connects an rmcp client to the `McpServer` in the
//! same process, driving the 12 tools through the actual JSON-RPC path. The
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
