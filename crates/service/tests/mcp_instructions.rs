//! In-process rmcp duplex tests for the routing block a server hands each
//! connecting agent as its `instructions`.
//!
//! A `tokio::io::duplex` pair connects an rmcp client to the `McpServer` in the
//! same process, driving the real JSON-RPC initialize handshake. The server's
//! `get_info` fills `instructions` from `Engine::routing_text`, and the client
//! reads them back through `peer_info().instructions`. The engine is started
//! against a real `config.yaml` on disk, so a domain registered after startup
//! is picked up by the same fresh-config re-read the production daemon does.
//!
//! The harness deliberately does not refresh the routing cache in `connect`:
//! the virtual-domain bullets appear only because `scaffold_virtual_manifest`
//! refreshes the cache itself, which is exactly the write-side hook under test.

use std::path::PathBuf;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use tokio::sync::Mutex;

/// A MANIFEST.md whose `## When to Use` section carries `bullets`, the routing
/// bullets `routing_text` reads for a domain. `permalink: manifest` so a virtual
/// domain's MANIFEST engram resolves by permalink the same way a file one does.
fn manifest_md(name: &str, bullets: &[&str]) -> String {
    let when: String = bullets.iter().map(|b| format!("- {b}\n")).collect();
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n{when}"
    )
}

struct Harness {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
    root: PathBuf,
    config_path: PathBuf,
    config: GlobalConfig,
}

impl Harness {
    /// Build a harness with the given file domains (each name paired with its
    /// `## When to Use` bullets) and virtual domains, its engine started against
    /// a real `config.yaml` on disk so `routing_text`'s post-startup re-read is
    /// exercised. `read_only` forces the engine's read-only mode.
    async fn build(
        file_domains: &[(&str, &[&str])],
        virtual_domains: &[&str],
        read_only: bool,
    ) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let config_path = root.join("config.yaml");
        let mut config = GlobalConfig::default();
        for (name, bullets) in file_domains {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("MANIFEST.md"), manifest_md(name, bullets)).unwrap();
            config
                .domains
                .insert(name.to_string(), DomainEntry::file(dir));
        }
        for name in virtual_domains {
            config
                .domains
                .insert(name.to_string(), DomainEntry::virtual_domain());
        }
        crystalline_core::config::save_yaml(&config_path, &config).unwrap();

        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(
                Arc::new(Mutex::new(store)),
                config.clone(),
                None,
                Some(config_path.clone()),
            )
            .with_read_only(read_only),
        );
        Harness {
            _tmp: tmp,
            engine,
            root,
            config_path,
            config,
        }
    }

    /// Open one rmcp connection and return the running client and server. The
    /// server handshake blocks until the client sends `initialize`, so the two
    /// must run concurrently.
    async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
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

/// The `instructions` string the server handed this client at initialize.
fn instructions(client: &RunningService<RoleClient, ()>) -> String {
    client
        .peer()
        .peer_info()
        .as_ref()
        .and_then(|i| i.instructions.clone())
        .unwrap_or_default()
}

/// A routing line per file domain, the header and the Behavior tool names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instructions_carry_a_routing_line_per_file_domain() {
    let h = Harness::build(
        &[
            ("eng", &["Route here for eng questions"]),
            ("ops", &["Route here for ops questions"]),
        ],
        &[],
        false,
    )
    .await;
    let (client, _server) = h.connect().await;
    let text = instructions(&client);

    let peer_info = client.peer().peer_info().unwrap();
    assert_eq!(peer_info.server_info.name, "crystalline");
    assert_eq!(peer_info.server_info.version, crystalline_core::VERSION);

    assert!(
        text.starts_with("CRYSTALLINE KNOWLEDGE ROUTING"),
        "header first:\n{text}"
    );
    assert!(
        text.contains("- eng: Route here for eng questions"),
        "eng routing line:\n{text}"
    );
    assert!(
        text.contains("- ops: Route here for ops questions"),
        "ops routing line:\n{text}"
    );
    assert!(text.contains("Behavior:"), "behavior block:\n{text}");
    for tool in [
        "search_engrams",
        "write_engram",
        "build_context",
        "read_engram",
        "list_domains",
    ] {
        assert!(text.contains(tool), "expected {tool} named:\n{text}");
    }
}

/// A virtual domain's bullets appear only after `scaffold_virtual_manifest`
/// writes its MANIFEST engram and refreshes the routing cache: the write-side
/// hook. Before the scaffold the routing line is the unavailable placeholder.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scaffolding_a_virtual_manifest_makes_its_bullets_appear() {
    let h = Harness::build(
        &[("eng", &["Route here for eng questions"])],
        &["notes"],
        false,
    )
    .await;

    let (client0, _s0) = h.connect().await;
    let before = instructions(&client0);
    assert!(
        before.contains("- notes: (routing information unavailable"),
        "placeholder before scaffold:\n{before}"
    );

    h.engine
        .scaffold_virtual_manifest(
            "notes",
            &manifest_md("notes", &["Route here for notes questions"]),
        )
        .await
        .unwrap();

    let (client1, _s1) = h.connect().await;
    let after = instructions(&client1);
    assert!(
        after.contains("- notes: Route here for notes questions"),
        "scaffolded bullets after the refresh hook:\n{after}"
    );
}

/// The read-only variant drops every content-mutating tool name and states the
/// knowledge is curated externally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_instructions_drop_the_write_tools() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], true).await;
    let (client, _server) = h.connect().await;
    let text = instructions(&client);

    assert!(
        text.contains("read-only and curated externally"),
        "read-only behavior line:\n{text}"
    );
    for tool in [
        "write_engram",
        "edit_engram",
        "move_engram",
        "delete_engram",
    ] {
        assert!(
            !text.contains(tool),
            "{tool} must be absent read-only:\n{text}"
        );
    }
    assert!(
        text.contains("search_engrams"),
        "read tool still named:\n{text}"
    );
}

/// A domain added to the config file after startup shows up on a new
/// connection, proving the fresh-config re-read on every `get_info`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_added_to_the_config_after_startup_appears_on_a_new_connection() {
    let mut h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;

    let (client0, _s0) = h.connect().await;
    let before = instructions(&client0);
    assert!(
        !before.contains("- extra:"),
        "extra not registered yet:\n{before}"
    );

    // Register a new file domain the way `domain add` does: edit the config file
    // on disk and give the domain a MANIFEST so its routing line has bullets.
    let extra_dir = h.root.join("extra");
    std::fs::create_dir_all(&extra_dir).unwrap();
    std::fs::write(
        extra_dir.join("MANIFEST.md"),
        manifest_md("extra", &["Route here for extra questions"]),
    )
    .unwrap();
    h.config
        .domains
        .insert("extra".to_string(), DomainEntry::file(extra_dir));
    crystalline_core::config::save_yaml(&h.config_path, &h.config).unwrap();

    let (client1, _s1) = h.connect().await;
    let after = instructions(&client1);
    assert!(
        after.contains("- extra: Route here for extra questions"),
        "the newly registered domain appears:\n{after}"
    );
}

/// A routing line shows at most three bullets even when the MANIFEST lists
/// more, keeping the instructions token-lean.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routing_lines_cap_at_three_bullets() {
    let h = Harness::build(
        &[("eng", &["one", "two", "three", "four", "five"])],
        &[],
        false,
    )
    .await;
    let (client, _server) = h.connect().await;
    let text = instructions(&client);

    let line = text
        .lines()
        .find(|l| l.starts_with("- eng:"))
        .expect("a routing line for eng");
    assert!(
        line.contains("one; two; three"),
        "first three kept:\n{line}"
    );
    assert!(
        !line.contains("four") && !line.contains("five"),
        "bullets past three dropped:\n{line}"
    );
}
