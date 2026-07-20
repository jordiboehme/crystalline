//! TOON vs JSON payload measurement: acceptance evidence for the response
//! format change, not a correctness test. Seeds realistic content, captures
//! each list-shaped tool's raw response text under both formats, and reports
//! byte and o200k-approximate token counts. The printed table (run with
//! `--no-capture`) is what gets recorded in the design doc; the assertion
//! only guards the minimum bar (TOON strictly smaller in bytes) in CI.

// The harness below is copied verbatim from mcp_tools.rs (integration test
// files are separate crates and cannot share it); this file only exercises
// `new_toon`, `new` and `connect`, so its unused pieces (`new_read_only`,
// `root`, the second half of the `connect` tuple) are allowed dead here
// rather than trimmed, to keep the copy a faithful one.
#![allow(dead_code)]

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
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
        Harness::build(domains, false, true).await
    }

    /// A harness whose engine serves the content API read-only.
    async fn new_read_only(domains: &[&str]) -> Harness {
        Harness::build(domains, true, true).await
    }

    /// A harness whose engine keeps the default TOON response format, for
    /// the dedicated format tests; every other test pins json so its
    /// assertions stay on data semantics over byte-identical JSON.
    async fn new_toon(domains: &[&str]) -> Harness {
        Harness::build(domains, false, false).await
    }

    async fn build(domains: &[&str], read_only: bool, pin_json: bool) -> Harness {
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
        if pin_json {
            cfg.service = Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                ..ServiceConfig::default()
            });
        }
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_read_only(read_only),
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

/// Call a tool, returning the raw text of its first content block on success.
async fn call_text(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<String, String> {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match peer.call_tool(params).await {
        Ok(result) => {
            let v = serde_json::to_value(&result).unwrap();
            Ok(v.pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Encodes representative payloads for the 4 list-shaped tools whose token
/// cost matters most in practice (search hits, activity feed, domain
/// listing, browse listing) both as TOON (the default) and as JSON (the
/// escape hatch), and prints a byte/token table for each. This is
/// measurement, not a correctness check; see mcp_tools.rs and the encoder's
/// own unit tests in crates/service/src/toon.rs for exact-output and
/// data-semantics coverage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toon_vs_json_payload_sizes() {
    let h = Harness::new_toon(&["eng", "ops"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    for i in 0..12 {
        let domain = if i % 2 == 0 { "eng" } else { "ops" };
        call(
            peer,
            "write_engram",
            json!({
                "domain": domain,
                "title": format!("Deploy note {i}"),
                "content": format!(
                    "Deploy note {i}: the rollout paused on the canary check, then resumed once the \
                     index rebuild finished. Watch the embed queue depth before scaling, and keep \
                     the daemon log open while the watcher settles."
                ),
                "tags": ["deploy", "canary", "runbook"],
            }),
        )
        .await
        .unwrap();
    }

    let payloads: &[(&str, Value)] = &[
        (
            "search_engrams",
            json!({ "query": "canary rollout", "limit": 10 }),
        ),
        ("recent_activity", json!({})),
        ("list_domains", json!({ "include_routing": true })),
        ("browse_domain", json!({ "domain": "eng" })),
    ];

    // Capture under the TOON default first.
    let mut toon_texts = Vec::new();
    for (tool, args) in payloads {
        let text = call_text(peer, tool, args.clone()).await.unwrap();
        toon_texts.push((*tool, text));
    }

    // Switch to json. This response itself still arrives as TOON (configure
    // is one of the 11 list-shaped tools and the switch applies from the
    // next call on), so the raw text is captured and ignored.
    call_text(
        peer,
        "configure",
        json!({ "set": { "service.response_format": "json" } }),
    )
    .await
    .unwrap();

    let mut json_texts = Vec::new();
    for (tool, args) in payloads {
        let text = call_text(peer, tool, args.clone()).await.unwrap();
        json_texts.push((*tool, text));
    }

    let bpe = tiktoken_rs::o200k_base().unwrap();
    println!(
        "\nTOON vs JSON payload measurement (o200k_base tokenizer, an approximation of the \
         client models' actual tokenizers - real savings vary by model):"
    );
    for ((name, toon_text), (_, json_text)) in toon_texts.iter().zip(json_texts.iter()) {
        let toon_b = toon_text.len();
        let json_b = json_text.len();
        let toon_tokens = bpe.encode_ordinary(toon_text).len();
        let json_tokens = bpe.encode_ordinary(json_text).len();
        println!(
            "{name}: json {json_b}B/{json_tokens}t toon {toon_b}B/{toon_tokens}t saving {:.0}%",
            100.0 * (1.0 - toon_tokens as f64 / json_tokens as f64)
        );
        assert!(
            toon_text.len() < json_text.len(),
            "{name}: TOON must not be larger than JSON: {toon_text}"
        );
    }
}
