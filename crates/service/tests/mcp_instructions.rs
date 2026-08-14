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
//!
//! The last section covers the receipt-aware variant: `connect_as` drives the
//! handshake with a chosen `clientInfo.name` over a chosen transport, against
//! a server pointed at a stand-in install receipt, which is exactly the three
//! inputs the `auto` value of `skills.serve` decides on.

use std::path::PathBuf;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::{ClientInfo, Implementation, ProtocolVersion};
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

    /// Open one connection whose client announces itself as `client_name` in
    /// the `initialize` handshake, against a server built for `transport` and
    /// pointed at this harness's own install receipt. That is everything the
    /// receipt-aware `auto` behaviour reads.
    async fn connect_as(
        &self,
        client_name: &str,
        transport: ServedTransport,
    ) -> (
        RunningService<RoleClient, ClientInfo>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let receipt = self.receipt_path();
        let server_task = tokio::spawn(async move {
            let server = match transport {
                ServedTransport::Stdio => McpServer::new(engine),
                ServedTransport::Http => McpServer::new_http(engine),
            };
            rmcp::serve_server(server.with_install_receipt(receipt), server_io).await
        });
        let mut info = ClientInfo::default();
        info.client_info = Implementation::new(client_name, "1.2.3");
        let client = rmcp::serve_client(info, client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }

    /// Open one connection whose client declares `version` in its `initialize`
    /// request, and return the protocol version the server answered with.
    async fn negotiate(&self, version: ProtocolVersion) -> ProtocolVersion {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let mut info = ClientInfo::default();
        info.protocol_version = version;
        let client = rmcp::serve_client(info, client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        let answered = client
            .peer()
            .peer_info()
            .as_ref()
            .map(|i| i.protocol_version.clone())
            .expect("the server answered initialize");
        drop(client);
        drop(server);
        answered
    }

    /// Where this harness's stand-in install receipt lives.
    fn receipt_path(&self) -> PathBuf {
        self.root.join("installs.json")
    }

    /// Write an install receipt in exactly the shape `crystalline install`
    /// writes, recording `harness` with `hooks` either wired or skipped.
    fn write_receipt(&self, harness: &str, hooks: bool) {
        std::fs::write(
            self.receipt_path(),
            format!(
                r#"{{"format":1,"installs":[{{"harness":"{harness}","scope":"user","version":"0.11.0","parts":{{"mcp":true,"hooks":{hooks},"skills":true}},"skills":[]}}]}}"#
            ),
        )
        .unwrap();
    }
}

/// Which transport a test wants its server built for. The `auto` gate applies
/// to stdio only.
#[derive(Clone, Copy)]
enum ServedTransport {
    Stdio,
    Http,
}

/// The `instructions` string the server handed this client at initialize.
fn instructions<H: rmcp::handler::client::ClientHandler>(
    client: &RunningService<RoleClient, H>,
) -> String {
    client
        .peer()
        .peer_info()
        .as_ref()
        .and_then(|i| i.instructions.clone())
        .unwrap_or_default()
}

/// The default TOON response format appends a note to the initialize
/// instructions so a client model reads list results as data; switching the
/// format to json drops the note for the next connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn instructions_note_toon_only_while_the_format_is_active() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    let (client, _server) = h.connect().await;
    let text = instructions(&client);
    assert!(
        text.contains("TOON"),
        "default instructions carry the TOON note:\n{text}"
    );
    drop(client);

    // Switching to json removes the note for the next connection.
    h.engine
        .configure(&crystalline_service::engine::ConfigureAction::Set {
            key: "service.response_format".to_string(),
            value: "json".to_string(),
        })
        .await
        .unwrap();
    let (client, _server) = h.connect().await;
    let text = instructions(&client);
    assert!(!text.contains("TOON"), "json mode drops the note:\n{text}");
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
    // `ServerPeerInfo::server_info` is optional because a discovery response
    // need not carry an identity (rmcp 3.1.2 `model.rs:1102`); an `initialize`
    // answer always does, so the unwrap is part of the assertion.
    let server_info = peer_info
        .server_info
        .as_ref()
        .expect("the server identified itself");
    assert_eq!(server_info.name, "crystalline");
    assert_eq!(server_info.version, crystalline_core::VERSION);

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

// --- receipt-aware instructions ---------------------------------------------
//
// One test per row of the `skills.serve` decision table. The full block runs
// to roughly 475 tokens of routing prose a locally installed harness has
// already received from its own SessionStart hook; the minimal block is the
// header plus one pointer sentence.

/// The row the whole feature exists for: a stdio client whose `initialize`
/// name is a harness this machine's receipt onboarded with hooks gets the
/// minimal block instead of the full routing prose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_matched_stdio_client_gets_the_minimal_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    h.write_receipt("claude-code", true);

    let (client, _server) = h.connect_as("claude-code", ServedTransport::Stdio).await;
    let text = instructions(&client);

    assert!(
        text.starts_with("CRYSTALLINE KNOWLEDGE ROUTING"),
        "the header still names what the server is:\n{text}"
    );
    assert!(
        text.contains("list_domains with include_routing=true"),
        "the pointer makes the full block one call away:\n{text}"
    );
    assert!(
        !text.contains("Behavior:") && !text.contains("- eng:"),
        "neither the behavior rules nor the routing lines are repeated:\n{text}"
    );
    assert!(
        text.contains("TOON"),
        "the wire-format note is not onboarding and stays:\n{text}"
    );
}

/// The same client over HTTP is never suppressed: a remote session says
/// nothing about what the machine running the client has on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_client_over_http_gets_the_full_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    h.write_receipt("claude-code", true);

    let (client, _server) = h.connect_as("claude-code", ServedTransport::Http).await;
    let text = instructions(&client);
    assert!(
        text.contains("Behavior:") && text.contains("- eng: Route here for eng questions"),
        "an HTTP session always gets the full routing block:\n{text}"
    );
}

/// A harness installed with `--skip-hooks` never receives the routing block at
/// session start, so the instructions must still carry it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hooks_skipped_install_still_gets_the_full_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    h.write_receipt("claude-code", false);

    let (client, _server) = h.connect_as("claude-code", ServedTransport::Stdio).await;
    let text = instructions(&client);
    assert!(
        text.contains("Behavior:") && text.contains("- eng: Route here for eng questions"),
        "no hooks means no other onboarding, so the block stays:\n{text}"
    );
}

/// An unrecognized client name and a machine with no receipt at all are both
/// misses, and a miss always means the full block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_client_or_a_missing_receipt_gets_the_full_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;

    // No receipt on disk yet.
    let (client, _server) = h.connect_as("claude-code", ServedTransport::Stdio).await;
    assert!(
        instructions(&client).contains("Behavior:"),
        "no receipt is no match"
    );
    drop(client);

    // A receipt that knows claude-code, but a different client connecting.
    h.write_receipt("claude-code", true);
    for name in ["mcp-inspector", "codex", "claude"] {
        let (client, _server) = h.connect_as(name, ServedTransport::Stdio).await;
        let text = instructions(&client);
        assert!(
            text.contains("Behavior:") && text.contains("- eng:"),
            "'{name}' is not a name an onboarded harness sends:\n{text}"
        );
    }
}

/// `skills.serve` forces the decision in both directions: `true` restores the
/// full block for a matched client, `false` leaves it alone entirely, since it
/// gates skill serving rather than onboarding.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_setting_overrides_the_receipt_in_both_directions() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    h.write_receipt("claude-code", true);

    for (value, expect_full) in [("true", true), ("false", true), ("auto", false)] {
        h.engine
            .configure(&crystalline_service::engine::ConfigureAction::Set {
                key: "skills.serve".to_string(),
                value: value.to_string(),
            })
            .await
            .unwrap();
        let (client, _server) = h.connect_as("claude-code", ServedTransport::Stdio).await;
        let text = instructions(&client);
        assert_eq!(
            text.contains("Behavior:"),
            expect_full,
            "skills.serve={value} should{} carry the full block:\n{text}",
            if expect_full { "" } else { " not" }
        );
    }
}

/// A client asking for a revision this server does not yet honour is answered
/// with the newest one it does. Today that is 2025-11-25: 2026-07-28 carries
/// obligations this server has not implemented, so echoing it back would be a
/// false claim. Ignored until the version block is rewritten to negotiate
/// against our own advertised set rather than rmcp's `KNOWN_VERSIONS`, which is
/// Task 2 of the rmcp 3.x migration plan.
#[ignore = "the advertised set lands in Task 2 of the rmcp 3.x migration"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_asking_for_an_unserved_protocol_version_is_answered_with_ours() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    assert_eq!(
        h.negotiate(ProtocolVersion::V_2026_07_28).await,
        ProtocolVersion::V_2025_11_25
    );
}
