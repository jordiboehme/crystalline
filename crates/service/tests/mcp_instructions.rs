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
//! The last sections cover the two things the block's delivery depends on:
//! the onboarding decision the spawned process resolved before the session
//! started (`connect_as`), and the arrival path each protocol revision uses.

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
        Harness::build_with(file_domains, virtual_domains, read_only, None).await
    }

    /// As [`Harness::build`], with `skills.serve` written into the config
    /// before the engine is built. That is the only way to change the value
    /// the server reads: it is snapshotted at engine construction, so a
    /// `configure` on a live engine saves the setting for the next start
    /// rather than moving anything now.
    async fn build_with(
        file_domains: &[(&str, &[&str])],
        virtual_domains: &[&str],
        read_only: bool,
        skills_serve: Option<&str>,
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
        if let Some(value) = skills_serve {
            crystalline_service::settings::apply(&mut config, "skills.serve", value).unwrap();
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

    /// Open one connection over `transport`, served with `onboarded` as the
    /// answer the spawned process resolved before the session started.
    ///
    /// **The HTTP arm ignores `onboarded`, and that is the asymmetry rather
    /// than a shortcut in the test.** One daemon serves every HTTP connection,
    /// a remote client never ran `crystalline install` on this machine and
    /// there is no per-client process to resolve anything, so the flag is
    /// never set there and an HTTP client is never suppressed.
    async fn connect_as(
        &self,
        onboarded: bool,
        transport: ServedTransport,
    ) -> (
        RunningService<RoleClient, ClientInfo>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let server_task = tokio::spawn(async move {
            let server = match transport {
                ServedTransport::Stdio => McpServer::new(engine).with_onboarded_harness(onboarded),
                ServedTransport::Http => McpServer::new_http(engine),
            };
            rmcp::serve_server(server, server_io).await
        });
        let mut info = ClientInfo::default();
        info.client_info = Implementation::new("mcp-test-client", "1.2.3");
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
}

/// Which transport a test wants its server built for. The `auto` gate applies
/// to stdio only.
#[derive(Clone, Copy)]
enum ServedTransport {
    Stdio,
    Http,
}

/// Build a `ProtocolVersion` from an arbitrary string, the way the wire does.
/// The type has no public constructor for unknown revisions, but its
/// `Deserialize` accepts any string (rmcp 3.1.2 `model.rs:204-220`), which is
/// exactly how a client's declared version reaches us.
fn protocol_version(s: &str) -> ProtocolVersion {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
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
//
// The decision's own inputs (the `--harness` argument and this machine's
// receipt) are resolved in the spawned process, so their table lives beside
// the resolver in `client.rs`. These tests take the resolved answer as given
// and prove what the server does with it.

/// The row the whole feature exists for: a session spawned by a harness this
/// machine has already onboarded gets the minimal block instead of the full
/// routing prose, because its own session hook delivered that block already.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_onboarded_harness_gets_the_minimal_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;

    let (client, _server) = h.connect_as(true, ServedTransport::Stdio).await;
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

/// An HTTP session is never suppressed, whatever this machine has installed:
/// one daemon serves every HTTP client, a remote client never ran
/// `crystalline install` here, and a remote client is exactly who the served
/// surface exists for. The `onboarded` argument is passed and deliberately
/// ignored on that transport.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_http_session_always_gets_the_full_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;

    let (client, _server) = h.connect_as(true, ServedTransport::Http).await;
    let text = instructions(&client);
    assert!(
        text.contains("Behavior:") && text.contains("- eng: Route here for eng questions"),
        "an HTTP session always gets the full routing block:\n{text}"
    );
}

/// Everything that is not a resolved, onboarded harness gets the full block:
/// a registration predating the `--harness` argument, an id this binary does
/// not know, a harness installed with `--skip-hooks`, a machine with no
/// receipt. Every one of those resolves to `false` in the bridge (see the
/// resolver's own table in `client.rs`), and `false` is this row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unresolved_session_gets_the_full_block() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;

    let (client, _server) = h.connect_as(false, ServedTransport::Stdio).await;
    let text = instructions(&client);
    assert!(
        text.contains("Behavior:") && text.contains("- eng: Route here for eng questions"),
        "nothing onboards this session, so the block stays:\n{text}"
    );
}

/// `skills.serve` forces the decision in both directions: `true` restores the
/// full block for an onboarded harness, `false` leaves it alone entirely,
/// since it gates skill serving rather than onboarding.
///
/// The three values are set in the config before the engine is built, where
/// this test used to `configure` them on a live engine between connections:
/// the effective value is snapshotted at engine construction now.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_skills_serve_setting_decides_in_both_directions() {
    for (value, expect_full) in [("true", true), ("false", true), ("auto", false)] {
        let h = Harness::build_with(
            &[("eng", &["Route here for eng questions"])],
            &[],
            false,
            Some(value),
        )
        .await;
        let (client, _server) = h.connect_as(true, ServedTransport::Stdio).await;
        let text = instructions(&client);
        assert_eq!(
            text.contains("Behavior:"),
            expect_full,
            "skills.serve={value} should{} carry the full block:\n{text}",
            if expect_full { "" } else { " not" }
        );
    }
}

/// **A handshake naming 2026-07-28 is answered with the newest revision that
/// still has a handshake, and that is upstream's rule rather than ours.**
///
/// The assertion read the other way while we were on rmcp 3.1.2, which echoed
/// any advertised revision back: a client could opt into the modern lifecycle
/// through `initialize` instead of opening with `server/discover`. rmcp 3.2.0
/// closed that path in `negotiate_protocol_version` (`service/server.rs:479`):
/// a requested revision is echoed only when it is a legacy one, otherwise the
/// server's newest legacy revision is returned. The reasoning is the one this
/// file already applies to an unknown version string - `initialize` is deleted
/// from the 2026-07-28 schema, so a peer that sent one is speaking the legacy
/// lifecycle whatever it names - and the era is now reached only the way the
/// specification provides for: `server/discover` and inline requests carrying
/// the SEP-2575 `_meta`, which `tests/mcp_modern_era.rs` covers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handshake_naming_the_era_is_answered_with_the_newest_handshake_revision() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    assert_eq!(
        h.negotiate(ProtocolVersion::V_2026_07_28).await,
        newest_handshake_revision(),
        "the era has no handshake, so one is answered with the newest that has"
    );
}

/// The newest revision we advertise that still carries an `initialize`
/// handshake, read off the advertised set rather than written as a literal so
/// a revision added or dropped moves it.
fn newest_handshake_revision() -> ProtocolVersion {
    crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS
        .iter()
        .rfind(|v| v.as_str() < "2026-07-28")
        .expect("we serve at least one revision with a handshake")
        .clone()
}

/// Every revision we advertise that has a handshake is echoed back verbatim,
/// oldest included, and the one that has none is answered with the newest that
/// does. Driven off the advertised set so a revision added without a decision
/// about the echo fails here.
///
/// 2024-11-05 is the bottom-end pin: it is served today, keeping it costs one
/// array element because rmcp branches nowhere between it and 2025-11-25
/// (`uses_legacy_lifecycle`, a single `<` against 2026-07-28), and dropping a
/// revision is a deprecation with a release note rather than a side effect of a
/// dependency bump.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_handshake_revision_we_serve_is_echoed_verbatim() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    for version in crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS {
        let expected = if version.as_str() < "2026-07-28" {
            version.clone()
        } else {
            newest_handshake_revision()
        };
        assert_eq!(
            h.negotiate(version.clone()).await,
            expected,
            "{version} over an initialize handshake"
        );
    }
}

/// `ProtocolVersion` deserializes any string (rmcp 3.1.2 `model.rs:204-220`
/// falls through to `Cow::Owned(s)`), so a client can declare a future-dated or
/// malformed revision. On stdio that is answered rather than refused: there is
/// no session routing to wedge, and a hard refusal would regress the day a
/// harness bumps its version string ahead of us.
///
/// **What it is answered with is not the newest revision we serve.** A client
/// that sent an `initialize` is speaking the legacy lifecycle, so it is
/// answered the newest revision that still has one; 2026-07-28 deletes the
/// handshake, and rmcp keys `ping`'s removal and the modern dispatch on the
/// negotiated version, so downgrading a client onto the era would take `ping`
/// away from a client that never asked for the era. Since rmcp 3.2.0 this is
/// upstream's rule as well as ours, and it applies to every revision without a
/// handshake rather than only to an unknown one - see
/// `a_handshake_naming_the_era_is_answered_with_the_newest_handshake_revision`.
/// A peer reaches the modern lifecycle through `server/discover` and inline
/// requests instead - `tests/mcp_modern_era.rs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_protocol_version_string_is_answered_with_the_newest_handshake_revision() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    for garbage in ["2027-01-01", "banana"] {
        assert_eq!(
            h.negotiate(protocol_version(garbage)).await,
            ProtocolVersion::V_2025_11_25,
            "{garbage} is not a revision anybody serves"
        );
    }
}

/// The advertised set, pinned deliberately.
///
/// An rmcp upgrade that adds or removes a revision must surface here as a
/// visible test change, never as silent drift: the second assertion pins
/// `ProtocolVersion::KNOWN_VERSIONS` (rmcp 3.1.2 `model.rs:181-187`) so a
/// widened crate list fails the build and forces the decision, and the first
/// pins what we actually advertise, which is spelled out literally in
/// `SERVED_PROTOCOL_VERSIONS` rather than filtered from the crate's list.
///
/// 2026-07-28 was added on 2026-08-14, once the four obligations it carries
/// were implemented and verified over both transports
/// (`tests/mcp_modern_era.rs`); 2024-11-05 is present on purpose and its
/// removal would be a deprecation, not a cleanup. The two lists are equal
/// today, and they are asserted separately on purpose: that is a fact about
/// this moment rather than a rule, and the day rmcp learns a sixth revision
/// only the second assertion should fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_advertised_protocol_set_is_exactly_this() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    let server = McpServer::new(h.engine.clone());
    assert_eq!(
        <McpServer as rmcp::ServerHandler>::supported_protocol_versions(&server).as_ref(),
        [
            ProtocolVersion::V_2024_11_05,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2026_07_28,
        ]
    );
    assert_eq!(
        ProtocolVersion::KNOWN_VERSIONS,
        [
            ProtocolVersion::V_2024_11_05,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2026_07_28,
        ],
        "rmcp's own list changed: decide which revisions we serve, then edit \
         SERVED_PROTOCOL_VERSIONS and this pin together"
    );
}

// --- the arrival path, per protocol revision ---------------------------------
//
// **The failure this section exists for is silent.** From 2026-07-28 there is
// no `initialize` and no `InitializeResult`: `instructions` moves to
// `DiscoverResult`, so a server that still only fills the handshake hands a
// modern agent nothing at all, with no error anywhere. An agent simply arrives
// uninstructed, which is the one outcome this product exists to prevent.
//
// So the table below is over the advertised set rather than over a list of
// eras somebody remembered to write down: adding a revision to
// `SERVED_PROTOCOL_VERSIONS` without giving it an arrival path fails here
// instead of shipping silence.
//
// **What this cannot prove**, stated so the test is not mistaken for coverage
// it does not provide: it proves this server offers the routing block by the
// path each era actually uses. It cannot prove a client chose to pull it - the
// specification explicitly permits a client never to call `server/discover`,
// and no server-side test can observe that decision. Only a by-hand run of a
// real agent over each era can, once.

/// How one protocol revision delivers the routing block.
#[derive(Debug, PartialEq, Eq)]
enum Arrival {
    /// `InitializeResult.instructions`, the four revisions below 2026-07-28.
    Initialize,
    /// `DiscoverResult.instructions`, from 2026-07-28 on, where the handshake
    /// is deleted from the schema outright.
    Discover,
}

/// The arrival path a revision uses. **Exhaustive on purpose**: a revision
/// this table does not know is a revision nobody has decided an onboarding
/// path for, and shipping it would be shipping the silent failure.
fn arrival_path(version: &ProtocolVersion) -> Arrival {
    match version.as_str() {
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => Arrival::Initialize,
        "2026-07-28" => Arrival::Discover,
        other => panic!(
            "protocol revision {other} is served with no onboarding path decided. \
             Give it one here and in McpServer before adding it to \
             SERVED_PROTOCOL_VERSIONS."
        ),
    }
}

/// The instructions a legacy peer reads out of its `initialize` answer.
async fn instructions_via_initialize(h: &Harness, version: ProtocolVersion) -> String {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let engine = h.engine.clone();
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(engine), server_io).await });
    let mut info = ClientInfo::default();
    info.protocol_version = version;
    let client = rmcp::serve_client(info, client_io).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    let text = instructions(&client);
    drop(client);
    drop(server);
    text
}

/// The instructions a modern peer reads out of its `server/discover` answer.
///
/// Driven through rmcp's own discover lifecycle rather than a hand-built
/// request, so the `_meta` the era requires (`protocolVersion` plus
/// `clientCapabilities`) is exactly what a real client would send.
async fn instructions_via_discover(h: &Harness, version: ProtocolVersion) -> String {
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let engine = h.engine.clone();
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(engine), server_io).await });
    let client = ClientInfo::default()
        .serve_with_lifecycle(
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![version],
            },
        )
        .await
        .expect("the server answered server/discover");
    let server = server_task.await.unwrap().unwrap();
    let text = instructions(&client);
    drop(client);
    drop(server);
    text
}

/// The routing block arrives by the path its era uses, for every revision this
/// server advertises.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_routing_block_arrives_by_every_served_revisions_own_path() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;

    let served = crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS;
    assert!(
        !served.is_empty(),
        "a server that serves nothing onboards nobody"
    );
    for version in served {
        let text = match arrival_path(version) {
            Arrival::Initialize => instructions_via_initialize(&h, version.clone()).await,
            Arrival::Discover => instructions_via_discover(&h, version.clone()).await,
        };
        assert!(
            text.starts_with("CRYSTALLINE KNOWLEDGE ROUTING"),
            "{version:?} received no routing block by its own arrival path:\n{text}"
        );
        assert!(
            text.contains("- eng: Route here for eng questions"),
            "{version:?} received a block with no routing lines in it:\n{text}"
        );
    }
}

/// The block itself is one block: whichever path an era uses, the bytes are
/// identical. `DiscoverResult::from_server_info` carries `instructions` out of
/// `ServerInfo` untouched (rmcp 3.1.2 `model.rs:1246-1268`), and both paths
/// build that `ServerInfo` through `McpServer::arrival_info`, so this pins
/// that neither ever grows a variant the other does not have.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_block_is_byte_identical_whichever_path_delivers_it() {
    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    let newest = crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS
        .last()
        .unwrap()
        .clone();

    let by_initialize = instructions_via_initialize(&h, newest.clone()).await;
    let by_discover = instructions_via_discover(&h, newest).await;
    assert_eq!(by_initialize, by_discover);
    assert!(!by_discover.is_empty());
}

/// The documented mitigation for a client that never calls `server/discover`,
/// pinned because it is the only thing standing between that client and no
/// onboarding at all. Both routes are pull-shaped, which is exactly the
/// weakness: they work, and they need the client to know to ask.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_never_discovers_can_still_pull_the_block() {
    use rmcp::model::{CallToolRequestParams, GetPromptRequestParams};

    let h = Harness::build(&[("eng", &["Route here for eng questions"])], &[], false).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let onboarding = peer
        .get_prompt(GetPromptRequestParams::new("onboarding"))
        .await
        .expect("the onboarding prompt answers");
    let text = serde_json::to_value(&onboarding).unwrap();
    let text = text
        .pointer("/messages/0/content/text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        text.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")
            && text.contains("- eng: Route here for eng questions"),
        "the onboarding prompt carries the live block:\n{text}"
    );

    let mut params = CallToolRequestParams::new("list_domains".to_string());
    params = params.with_arguments(
        serde_json::json!({ "include_routing": true })
            .as_object()
            .unwrap()
            .clone(),
    );
    let listed = peer.call_tool(params).await.expect("list_domains answers");
    let listed = serde_json::to_value(&listed).unwrap().to_string();
    assert!(
        listed.contains("Route here for eng questions"),
        "list_domains with include_routing carries the same routing text:\n{listed}"
    );
}
