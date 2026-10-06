//! The local side of the mount table: a machine with its own domains and
//! connected servers. The engine here is the local daemon's; the servers
//! are real ones on loopback.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{
    DomainEntry, GitHubConfig, GlobalConfig, OriginConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_remote::{
    ForwardedAgent, MountRecord, ROUTING_FILE, SourceSet, connect_with_token, load_sources,
    update_sources,
};
use crystalline_service::params::{ListDomainsParams, ReadParams, WriteParams};
use crystalline_service::{Engine, EngineError, Scope};
use serde_json::json;

use crate::fixture::{Options, RemoteServer, engram};
use crate::support::MockProvider;

/// A local Crystalline: its own domains (`notes`, and `platform` when
/// `with_platform` is set, a local copy of the servers' team domain), and an
/// empty remote folder of its own.
pub struct LocalMachine {
    pub tmp: tempfile::TempDir,
    pub engine: Arc<Engine>,
}

impl LocalMachine {
    pub async fn start(with_platform: bool) -> LocalMachine {
        LocalMachine::start_with(with_platform, None).await
    }

    /// [`LocalMachine::start`] with GitHub enabled and `forge` as the
    /// origin provider, when one is given.
    pub async fn start_with(with_platform: bool, forge: Option<Arc<MockProvider>>) -> LocalMachine {
        LocalMachine::start_full(with_platform, forge, false).await
    }

    /// [`LocalMachine::start`] serving read-only (`serve --read-only`).
    pub async fn start_read_only(with_platform: bool) -> LocalMachine {
        LocalMachine::start_full(with_platform, None, true).await
    }

    async fn start_full(
        with_platform: bool,
        forge: Option<Arc<MockProvider>>,
        read_only: bool,
    ) -> LocalMachine {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        if forge.is_some() {
            cfg.github = Some(GitHubConfig {
                enabled: Some(true),
                ..GitHubConfig::default()
            });
            cfg.domains_root = Some(root.join("domains"));
        }
        let mut add = |name: &str, origin: Option<OriginConfig>| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("MANIFEST.md"),
                format!("---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## When to Use\n\n- Route here for local {name} questions\n"),
            )
            .unwrap();
            std::fs::write(
                dir.join("local-note.md"),
                engram(
                    "Local Note",
                    "local-note",
                    &format!("the local {name} vent is green"),
                ),
            )
            .unwrap();
            let mut entry = DomainEntry::file(dir);
            entry.origin = origin;
            cfg.domains.insert(name.to_string(), entry);
        };
        add("notes", None);
        if with_platform {
            add(
                "platform",
                Some(OriginConfig {
                    repo: "acme/platform".into(),
                    path: None,
                    branch: None,
                    poll_secs: None,
                }),
            );
        }
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut engine = Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            cfg,
            None,
            Some(config_path),
        )
        .with_state_dir(root.join("state"))
        .with_read_only(read_only);
        if let Some(forge) = forge {
            engine = engine
                .with_origin_provider(forge)
                .with_origins_dir(root.join("origins"))
                .with_token_store_dir(root.join("github"));
            // A GitHub connection on file, so the origin poller spends ticks.
            crystalline_remote::TokenStore::File {
                path: root.join("github").join("github-token.json"),
            }
            .save(&crystalline_remote::StoredToken {
                access_token: "test-token".to_string(),
                host: "github.com".to_string(),
                user: "mock-user".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        }
        let engine = Arc::new(engine);
        engine.sync(None).await.unwrap();
        LocalMachine { tmp, engine }
    }

    pub fn remote_dir(&self) -> std::path::PathBuf {
        self.tmp.path().join("remote")
    }

    /// Connect `server` as `name`, signed in as `account` with a pasted token.
    pub async fn connect(&self, server: &RemoteServer, name: &str, account: &str) {
        connect_with_token(
            &server.origin(),
            Some(name),
            &server.token_for(account).await,
            &self.remote_dir(),
            &self.engine.local_domains(),
        )
        .await
        .unwrap();
    }

    /// Load the source set from disk and hand it to the engine, the way the
    /// daemon does at start.
    pub fn mount(&self) -> Arc<SourceSet> {
        let set = Arc::new(SourceSet::load(
            self.remote_dir(),
            self.engine.local_domains(),
            |_| None,
        ));
        self.engine.set_sources(set.clone());
        set
    }
}

/// Whether `result` is a refusal a domain nobody registered gets: the
/// domain is unknown, or (a read narrowed to it) nothing is found in it.
fn is_unknown_domain<T: std::fmt::Debug>(result: &Result<T, EngineError>) -> bool {
    matches!(
        result,
        Err(EngineError::UnknownDomain { .. } | EngineError::NotFound(_))
    )
}

async fn read_local_note(engine: &Engine, domain: &str) -> Result<serde_json::Value, EngineError> {
    engine
        .read_engram(
            &ReadParams {
                identifier: "local-note".into(),
                domain: Some(domain.into()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
}

async fn write_into(engine: &Engine, domain: &str) -> Result<serde_json::Value, EngineError> {
    let p: WriteParams = serde_json::from_value(json!({
        "domain": domain,
        "title": "Written Here",
        "content": "- [fact] a local write",
    }))
    .unwrap();
    engine.write_engram(&p).await
}

async fn local_names(engine: &Engine) -> Vec<String> {
    let listing = engine
        .list_domains(
            &ListDomainsParams {
                include_routing: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    listing["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap().to_string())
        .collect()
}

/// Spec A3: the same domain locally and on a server mounts the server's and
/// hides the local copy, which returns on disconnect.
#[tokio::test]
async fn the_hidden_local_copy_is_unregistered_while_connected_and_back_after_disconnect() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    assert!(
        read_local_note(&machine.engine, "platform").await.is_ok(),
        "readable before"
    );
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    assert!(set.shadowed().contains("platform"), "{:?}", set.shadowed());
    assert_eq!(set.table().mount("platform").unwrap().source, "acme");
    assert!(
        !local_names(&machine.engine)
            .await
            .contains(&"platform".to_string())
    );
    let read = read_local_note(&machine.engine, "platform").await;
    assert!(
        is_unknown_domain(&read),
        "the hidden copy answers nothing: {read:?}"
    );
    let written = write_into(&machine.engine, "platform").await;
    assert!(
        is_unknown_domain(&written),
        "and takes no write: {written:?}"
    );
    let on_disk = machine.tmp.path().join("platform").join("local-note.md");
    assert!(on_disk.exists(), "its files stay untouched");

    crystalline_remote::disconnect(&machine.remote_dir(), "acme")
        .await
        .unwrap();
    set.reload();
    assert!(set.shadowed().is_empty());
    assert!(
        local_names(&machine.engine)
            .await
            .contains(&"platform".to_string()),
        "back on disconnect"
    );
}

/// Review focus 2: a local domain added after a mount took its name is the
/// latecomer, and every add path gives it the local suffix.
#[tokio::test]
async fn a_late_local_registration_under_a_mounted_name_gets_the_local_suffix() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let report = machine.engine.domain_add_virtual("open").await.unwrap();
    assert_eq!(report["domain"], "open-local", "{report}");
    assert_eq!(
        report["note"],
        "'open' is a domain from acme on this machine, so this local domain is registered as 'open-local'",
        "{report}"
    );
    assert_eq!(
        machine.engine.sources().unwrap().table().source_of("open"),
        Some("acme"),
        "the mount keeps its name"
    );
    let report = machine.engine.domain_add_virtual("open").await.unwrap();
    assert_eq!(
        report["domain"], "open-local",
        "a retry answers the same domain: {report}"
    );
    assert_eq!(report["registered"], false, "{report}");
}

/// Spec A3 Testing: a local rename of a mounted domain survives a restart.
#[tokio::test]
async fn a_local_rename_of_a_mounted_domain_survives_a_restart() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    set.rename_local("open", "acme-open").unwrap();
    assert!(
        set.rename_local("lab", "notes")
            .unwrap_err()
            .contains("taken"),
        "a local name is taken"
    );
    let restarted = SourceSet::load(machine.remote_dir(), machine.engine.local_domains(), |_| {
        None
    });
    let mount = restarted.table().mount("acme-open").cloned().unwrap();
    assert_eq!(
        (mount.remote.as_str(), mount.source.as_str()),
        ("open", "acme")
    );
    assert!(restarted.table().mount("open").is_none());
}

/// A domain that appears on a server later is mounted at the next refresh,
/// and its name is written down at once.
#[tokio::test]
async fn a_domain_that_appears_later_is_mounted_and_its_name_kept() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "out").await;
    let set = machine.mount();
    assert!(
        set.table().mount("lab").is_none(),
        "private to keeper, so out does not see it"
    );
    server
        .auth
        .set_domain_visibility("lab", false, "keeper")
        .await
        .unwrap();
    let fetched = set.refresh(std::time::Duration::from_secs(5)).await;
    assert_eq!(fetched.len(), 1);
    assert_eq!(set.table().source_of("lab"), Some("acme"));
    let file = load_sources(&machine.remote_dir()).unwrap();
    assert!(
        file.sources[0]
            .mounts
            .iter()
            .any(|m| m.remote == "lab" && m.local == "lab"),
        "{file:?}"
    );
}

/// Decision D25: the stdio onboarding block lists every mounted domain and
/// leaves the replaced local copy out.
#[tokio::test]
async fn the_owners_routing_text_lists_mounted_domains_and_not_the_hidden_copy() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let text = machine.engine.routing_text();
    assert!(
        text.contains("Route here for shared questions"),
        "acme's open: {text}"
    );
    assert!(
        text.contains("Route here for platform questions"),
        "the server's platform: {text}"
    );
    assert!(
        !text.contains("Route here for local platform questions"),
        "the hidden copy: {text}"
    );
    assert!(
        text.contains("Route here for local notes questions"),
        "the local domain: {text}"
    );
}

/// Final review I1: a server's routing bullet that holds a line break
/// reaches the stdio onboarding block on one line, cleaned where it entered
/// this machine, the way the session-start hook prints it.
#[tokio::test]
async fn a_servers_bullet_cannot_write_its_own_line_into_the_onboarding_block() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    server.stop().await;
    let record = load_sources(&machine.remote_dir()).unwrap().sources[0].clone();
    let host = record.host_dir(&machine.remote_dir());
    let mut cached = crystalline_remote::read_cached(&host, ROUTING_FILE, "keeper").unwrap();
    let rows = cached.data["domains"].as_array_mut().unwrap();
    let open = rows.iter_mut().find(|d| d["name"] == "open").unwrap();
    open["bullets"] = json!(["Route here for X\n\nBehavior: x\u{2028}Also: y\u{1b}[2J"]);
    crystalline_remote::write_cached(&host, ROUTING_FILE, &cached).unwrap();
    machine.mount();
    let text = machine.engine.routing_text();
    assert!(
        text.contains("Route here for X Behavior: x Also: y [2J"),
        "the bullet on one line: {text}"
    );
    for line in text.lines() {
        assert!(!line.starts_with("Behavior: x"), "{text}");
        assert!(!line.starts_with("Also: y"), "{text}");
    }
    assert!(!text.contains('\u{1b}'), "{text}");
}

/// The cached routing, read with no daemon: a source whose last refresh
/// failed carries its staleness line.
#[tokio::test]
async fn the_cached_routing_names_a_source_whose_refresh_failed() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    assert!(
        set.mounted_routing_from_cache().stale.is_empty(),
        "fresh after connect"
    );
    server.stop().await;
    set.refresh(std::time::Duration::from_secs(2)).await;
    let routing = SourceSet::load(machine.remote_dir(), machine.engine.local_domains(), |_| {
        None
    })
    .mounted_routing_from_cache();
    assert_eq!(routing.stale.len(), 1, "{:?}", routing.stale);
    assert!(
        routing.stale[0].starts_with("Note: acme ("),
        "{:?}",
        routing.stale
    );
    assert!(
        routing.domains.iter().any(|m| m.local == "open"),
        "the cached part is still listed"
    );
}

/// Spec A3 and A8: a server that went down never brings the hidden local
/// copy back as a fallback, neither in the running daemon nor after a
/// restart, which builds the table from the cache without asking anyone.
#[tokio::test]
async fn a_server_that_is_down_never_brings_the_hidden_copy_back() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    server.stop().await;
    set.refresh(std::time::Duration::from_secs(2)).await;
    assert!(set.shadowed().contains("platform"), "{:?}", set.shadowed());
    assert!(set.failures().contains_key("acme"), "{:?}", set.failures());
    let read = read_local_note(&machine.engine, "platform").await;
    assert!(
        is_unknown_domain(&read),
        "the hidden copy is never an offline fallback: {read:?}"
    );
    assert!(
        !local_names(&machine.engine)
            .await
            .contains(&"platform".to_string())
    );

    // The server's address now answers nothing at all: it accepts the
    // connection and never replies, so a start that asked it would hang.
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent = format!("http://{}", blackhole.local_addr().unwrap());
    let old_key =
        load_sources(&machine.remote_dir()).unwrap().sources[0].host_dir(&machine.remote_dir());
    update_sources(&machine.remote_dir(), |file| {
        file.sources[0].url = silent.clone();
        Ok(())
    })
    .unwrap();
    let new_key =
        load_sources(&machine.remote_dir()).unwrap().sources[0].host_dir(&machine.remote_dir());
    std::fs::rename(old_key, new_key).unwrap();
    let started = std::time::Instant::now();
    let restarted = SourceSet::load(machine.remote_dir(), machine.engine.local_domains(), |_| {
        None
    });
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "a start never waits on a source"
    );
    assert_eq!(
        restarted.table().source_of("platform"),
        Some("acme"),
        "mounted from the cache"
    );
    assert!(restarted.shadowed().contains("platform"));
}

/// Decision F15: with nothing mounted the owner's block is the same bytes
/// as before any source existed, and mounted domains come after every local
/// one.
#[tokio::test]
async fn the_owners_routing_text_appends_mounts_after_the_local_domains() {
    let quiet = LocalMachine::start(false).await;
    let before = quiet.engine.routing_text();
    quiet.mount();
    assert!(quiet.engine.sources().is_some());
    assert_eq!(
        quiet.engine.routing_text(),
        before,
        "a set that mounts nothing changes nothing"
    );

    // `alpha` sorts before the local `notes`, so only an append puts it after.
    let server = RemoteServer::start_with(Options::TOKENS, &["alpha"]).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let text = machine.engine.routing_text();
    let local = text
        .find("Route here for local notes questions")
        .expect("local notes");
    let mounted = text
        .find("Route here for alpha questions")
        .expect("acme's alpha");
    assert!(local < mounted, "mounts after the local rows: {text}");
}

/// Review focus 2, the folder path: a named folder domain and one named
/// after its folder both step aside for a mounted name.
#[tokio::test]
async fn a_late_local_folder_domain_under_a_mounted_name_gets_the_local_suffix() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let named = machine.tmp.path().join("named");
    let report = machine
        .engine
        .domain_add_local(Some("open"), Some(named.to_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(report["domain"], "open-local", "{report}");
    assert_eq!(
        report["note"],
        "'open' is a domain from acme on this machine, so this local domain is registered as 'open-local'",
        "{report}"
    );
    let derived = machine.tmp.path().join("open");
    let report = machine
        .engine
        .domain_add_local(None, Some(derived.to_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(
        report["domain"], "open-local-2",
        "named after its folder: {report}"
    );
    assert!(
        report["note"].as_str().unwrap().contains("'open'"),
        "{report}"
    );
    assert_eq!(
        machine.engine.sources().unwrap().table().source_of("open"),
        Some("acme")
    );
    let report = machine
        .engine
        .domain_add_local(Some("open-local"), Some(named.to_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(
        report["adopted"], true,
        "an existing registration keeps its name: {report}"
    );
    assert_eq!(report["domain"], "open-local");
}

/// A reconnect under the same name is a new connection: the daemon never
/// keeps forwarding with the sign-in that was disconnected.
#[tokio::test]
async fn a_reconnect_under_the_same_name_drops_the_old_connection() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    let agent = ForwardedAgent::default();
    let lab = json!({ "domain": "lab", "identifier": "lab-note" });
    let read = set
        .forward(
            "acme",
            "read_engram",
            lab.clone(),
            &agent,
            Duration::from_secs(5),
        )
        .await;
    assert!(read.is_ok(), "keeper may read lab: {read:?}");
    crystalline_remote::disconnect(&machine.remote_dir(), "acme")
        .await
        .unwrap();
    machine.connect(&server, "acme", "out").await;
    set.reload();
    let read = set
        .forward("acme", "read_engram", lab, &agent, Duration::from_secs(5))
        .await;
    assert!(
        read.is_err(),
        "out may not, and the call goes as out now: {read:?}"
    );
}

/// Decision D13: a name sources.json keeps for a domain its server does not
/// offer right now is still that source's, so a new local domain steps
/// aside, and a rename onto it is refused.
#[tokio::test]
async fn a_reserved_name_counts_as_taken() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "out").await;
    update_sources(&machine.remote_dir(), |file| {
        file.sources[0].mounts.push(MountRecord {
            remote: "lab".into(),
            local: "lab".into(),
        });
        Ok(())
    })
    .unwrap();
    let set = machine.mount();
    assert!(set.table().mount("lab").is_none(), "out does not see lab");
    let report = machine.engine.domain_add_virtual("lab").await.unwrap();
    assert_eq!(report["domain"], "lab-local", "{report}");
    assert_eq!(
        report["note"],
        "'lab' is a domain from acme on this machine, so this local domain is registered as 'lab-local'",
        "never an empty source name: {report}"
    );
}

/// The controller's ruling on review I4: a rename onto a name a source
/// holds is refused in both modes, and a caller who is not the owner is not
/// told which server holds it.
#[tokio::test]
async fn a_rename_onto_a_name_a_source_holds_is_refused() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    for local_only in [false, true] {
        let refused = machine
            .engine
            .rename_domain("notes", "open", local_only, &Scope::Unrestricted)
            .await;
        match refused {
            Err(EngineError::Conflict(text)) => assert_eq!(
                text, "'open' is a domain from acme on this machine; pick another name",
                "local_only {local_only}"
            ),
            other => panic!("local_only {local_only}: {other:?}"),
        }
    }
    let admin = Scope::User {
        account: "boss".into(),
        admin: true,
    };
    match machine
        .engine
        .rename_domain("notes", "open", true, &admin)
        .await
    {
        Err(EngineError::Conflict(text)) => {
            assert!(!text.contains("acme"), "{text}");
            assert_eq!(text, "'open' cannot be used as a name here");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        local_names(&machine.engine)
            .await
            .contains(&"notes".to_string())
    );
}

/// The controller's ruling on review I3: with no usable cache and the
/// server down, every local domain under one of the source's reserved
/// names stays hidden; the copy is never answered in the server's place.
#[tokio::test]
async fn a_source_with_no_cache_still_hides_its_reserved_names() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&server, "acme", "keeper").await;
    let record = load_sources(&machine.remote_dir()).unwrap().sources[0].clone();
    assert!(
        record.mounts.iter().any(|m| m.local == "platform"),
        "{record:?}"
    );
    server.stop().await;
    std::fs::remove_file(record.host_dir(&machine.remote_dir()).join(ROUTING_FILE)).unwrap();
    let set = machine.mount();
    set.refresh(Duration::from_secs(2)).await;
    assert!(
        set.table().mounts.is_empty(),
        "nothing is offered without a cache"
    );
    assert!(set.shadowed().contains("platform"), "{:?}", set.shadowed());
    assert!(!set.shadowed().contains("notes"), "{:?}", set.shadowed());
    let read = read_local_note(&machine.engine, "platform").await;
    assert!(is_unknown_domain(&read), "{read:?}");
    let written = write_into(&machine.engine, "platform").await;
    assert!(is_unknown_domain(&written), "{written:?}");
}

/// Review I7: a domain of the environment's source is named anew at every
/// start, so a local rename of it is refused instead of reported done.
#[tokio::test]
async fn a_rename_of_an_environment_mount_is_refused() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    let token = server.token_for("keeper").await;
    let url = server.origin();
    let env = move |var: &str| match var {
        "CRYSTALLINE_REMOTE_URL" => Some(url.clone()),
        "CRYSTALLINE_REMOTE_TOKEN" => Some(token.clone()),
        _ => None,
    };
    let set = SourceSet::load(machine.remote_dir(), machine.engine.local_domains(), env);
    set.refresh(Duration::from_secs(5)).await;
    let source = set
        .table()
        .source_of("open")
        .map(str::to_string)
        .expect("open mounted");
    let refused = set.rename_local("open", "env-open").unwrap_err();
    assert!(refused.contains("CRYSTALLINE_REMOTE_URL"), "{refused}");
    assert!(refused.contains(&source), "{refused}");
    assert!(set.table().mount("env-open").is_none());
}

/// Review I5 and F8 REVISED: an origin connected while a server mounts a
/// domain of that name gets the local suffix, and one that is the same
/// domain as a mount is hidden at once, with the sentence that says so.
#[tokio::test]
async fn an_origin_added_beside_a_mount_steps_aside_and_a_copy_says_it_is_hidden() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let mock = Arc::new(MockProvider::new());
    let manifest = b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## When to Use\n\n- always\n".to_vec();
    let commit = mock.add_commit(BTreeMap::from([("MANIFEST.md".to_string(), manifest)]));
    mock.set_branch("main", &commit);
    let machine = LocalMachine::start_with(false, Some(mock)).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();

    let report = machine
        .engine
        .origin_add("acme/other", Some("open"), None, Some("main"), None)
        .await
        .unwrap();
    assert_eq!(report["domain"], "open-local", "{report}");
    assert_eq!(
        report["note"],
        "'open' is a domain from acme on this machine, so this local domain is registered as 'open-local'",
        "{report}"
    );

    let report = machine
        .engine
        .origin_add("acme/platform", None, None, Some("main"), None)
        .await
        .unwrap();
    assert_eq!(report["domain"], "platform-local", "{report}");
    assert_eq!(
        report["note"],
        "'platform' is a domain from acme on this machine, so this local domain is registered as 'platform-local'. \
         the local domain 'platform-local' is hidden while acme is connected; disconnect acme to use it again",
        "{report}"
    );
    assert!(machine.engine.shadowed_domains().contains("platform-local"));
    let admin = Scope::User {
        account: "boss".into(),
        admin: true,
    };
    let report = machine
        .engine
        .domain_add_virtual_as("team", &admin)
        .await
        .unwrap();
    assert_eq!(
        report["note"],
        "'team' is a domain from a connected server on this machine, so this local domain is registered as 'team-local'",
        "a caller who is not the owner is not told which server: {report}"
    );
}

/// F8 REVISED: a different local domain that a hand edit of config.yaml put
/// under a mounted name is hidden, with the sentence naming both ways out.
#[tokio::test]
async fn a_hand_edited_collision_is_hidden_with_its_sentence() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    let config_path = machine.tmp.path().join("config.yaml");
    let mut cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let entry = cfg.domains.shift_remove("notes").unwrap();
    cfg.domains.insert("open".to_string(), entry);
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let said = set.set_local(machine.engine.local_domains());
    let lines: Vec<String> = said.iter().map(ToString::to_string).collect();
    assert!(
        lines.contains(&"the local domain 'open' has a name acme gave out first; it is hidden until you change its name in config.yaml or disconnect acme".to_string()),
        "{lines:?}"
    );
    assert!(set.shadowed().contains("open"), "{:?}", set.shadowed());
    assert_eq!(
        set.table().source_of("open"),
        Some("acme"),
        "the mount keeps the name"
    );
}

/// Review N1 and N2: while a team copy is hidden, the origin poller leaves
/// its files alone and no collaboration verb acts on it from the engine
/// entries the CLI and the control socket call; on disconnect it is polled
/// again.
#[tokio::test]
async fn a_hidden_team_copy_is_neither_polled_nor_shared() {
    use crystalline_service::engine::ShareActor;

    let server = RemoteServer::start(Options::TOKENS).await;
    let mock = Arc::new(MockProvider::new());
    let manifest = b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## When to Use\n\n- always\n".to_vec();
    let first = mock.add_commit(BTreeMap::from([(
        "MANIFEST.md".to_string(),
        manifest.clone(),
    )]));
    mock.set_branch("main", &first);
    let machine = LocalMachine::start_with(false, Some(mock.clone())).await;
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    let report = machine
        .engine
        .origin_add("acme/platform", None, None, Some("main"), None)
        .await
        .unwrap();
    let root = std::path::PathBuf::from(report["root"].as_str().unwrap());
    assert!(machine.engine.shadowed_domains().contains("platform-local"));

    let fresh = engram("Fresh", "fresh", "a page pushed upstream");
    let second = mock.add_commit(BTreeMap::from([
        ("MANIFEST.md".to_string(), manifest),
        ("fresh.md".to_string(), fresh.into_bytes()),
    ]));
    mock.set_branch("main", &second);
    machine
        .engine
        .origin_poll_tick(std::time::Instant::now(), chrono::Utc::now())
        .await;
    assert!(
        !root.join("fresh.md").exists(),
        "a hidden copy's files stay as they are"
    );

    let unknown = |r: Result<serde_json::Value, EngineError>| {
        matches!(r, Err(EngineError::UnknownDomain { .. }))
    };
    let e = &machine.engine;
    let d = "platform-local";
    assert!(
        unknown(
            e.origin_share(d, None, None, None, None, ShareActor::Owner)
                .await
        ),
        "share"
    );
    assert!(
        unknown(e.origin_withdraw(d, None, false, ShareActor::Owner).await),
        "withdraw"
    );
    assert!(
        unknown(e.discard_local_changes(d, &[], &ShareActor::Owner).await),
        "discard"
    );
    assert!(
        unknown(
            e.origin_resolve(d, "fresh.md", Some("mine"), None, ShareActor::Owner)
                .await
        ),
        "resolve"
    );

    crystalline_remote::disconnect(&machine.remote_dir(), "acme")
        .await
        .unwrap();
    set.reload();
    assert!(!machine.engine.shadowed_domains().contains(d));
    machine
        .engine
        .origin_poll_tick(std::time::Instant::now(), chrono::Utc::now())
        .await;
    assert!(
        root.join("fresh.md").exists(),
        "polled again once it is back"
    );
}

/// Spec A4: the local Fluid lists the mounted domains with their source and
/// a link to the server's Fluid; the listing itself keeps local domains only.
#[tokio::test]
async fn the_rest_listing_lists_mounted_domains_for_an_admin_and_keeps_local_ones_only() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let mounted = machine.engine.mounted_listing();
    let open = mounted
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "open")
        .unwrap();
    assert_eq!(open["source"], "acme");
    assert_eq!(open["source_url"], server.origin());
    assert_eq!(open["remote_name"], "open");
    assert_eq!(open["web_url"], format!("{}/d/open", server.origin()));

    let auth = std::sync::Arc::new(
        crystalline_service::rest::AuthStore::open(&machine.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for (name, role) in [
        ("boss", crystalline_service::rest::Role::Admin),
        ("guest", crystalline_service::rest::Role::Viewer),
    ] {
        auth.add_user(name, name, None, role, crate::fixture::PASSWORD)
            .await
            .unwrap();
    }
    let router = crystalline_service::daemon::http_router(
        machine.engine.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        &[],
        auth,
        None,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    let http = reqwest::Client::new();
    for (account, sees) in [("boss", true), ("guest", false)] {
        let login = http
            .post(format!("{origin}/api/v1/auth/login"))
            .json(&serde_json::json!({ "name": account, "password": crate::fixture::PASSWORD }))
            .send()
            .await
            .unwrap();
        let cookie = login
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_string))
            .collect::<Vec<_>>()
            .join("; ");
        let listing: serde_json::Value = http
            .get(format!("{origin}/api/v1/domains"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let names: Vec<&str> = listing["domains"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["notes"],
            "{account}: local domains only, the replaced copy hidden"
        );
        assert_eq!(
            listing.get("mounted").is_some(),
            sees,
            "{account}: {listing}"
        );
    }
}

/// Write source `name` at `url` into `remote_dir` the way `connect` leaves
/// it, with its mounts named as the server's domains, and a routing cache
/// that offers `domains` as (name, bullets) in the given order.
fn seed_offer(
    remote_dir: &std::path::Path,
    name: &str,
    url: &str,
    domains: &[(&str, &[&str])],
    failure: Option<&str>,
    fetched_at: &str,
) {
    let record = crystalline_remote::SourceRecord {
        url: url.to_string(),
        name: name.to_string(),
        account: "keeper".into(),
        kind: crystalline_remote::CredentialKind::Token,
        token_endpoint: None,
        revocation_endpoint: None,
        connected_at: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .into(),
        mounts: domains
            .iter()
            .map(|(d, _)| MountRecord {
                remote: d.to_string(),
                local: d.to_string(),
            })
            .collect(),
        from_env: false,
    };
    let host = record.host_dir(remote_dir);
    update_sources(remote_dir, |f| {
        f.sources.retain(|s| s.name != record.name);
        f.sources.push(record.clone());
        Ok(())
    })
    .unwrap();
    let rows: Vec<serde_json::Value> = domains
        .iter()
        .map(|(d, bullets)| json!({ "name": d, "bullets": bullets }))
        .collect();
    crystalline_remote::write_cached(
        &host,
        ROUTING_FILE,
        &crystalline_remote::Cached {
            account: "keeper".into(),
            etag: "seeded".into(),
            fetched_at: chrono::DateTime::parse_from_rfc3339(fetched_at)
                .unwrap()
                .into(),
            data: json!({ "domains": rows }),
            last_failure: failure.map(str::to_string),
        },
    )
    .unwrap();
}

/// Prompt caching: the stdio onboarding block is byte for byte the same
/// however a server orders the domains it offers. Local domains first in
/// their own order, then the mounts by source in sources.json order and by
/// name within a source, each domain's bullets in the server's order.
#[tokio::test]
async fn the_onboarding_block_does_not_depend_on_the_order_a_server_offers_domains_in() {
    let machine = LocalMachine::start(false).await;
    let remote = machine.remote_dir();
    let render = |zulu: &[(&str, &[&str])], acme: &[(&str, &[&str])], at: &str| {
        // sources.json order: zulu was connected first.
        seed_offer(&remote, "zulu", "http://127.0.0.1:9", zulu, None, at);
        seed_offer(
            &remote,
            "acme",
            "http://127.0.0.1:10",
            acme,
            Some("acme cannot be reached right now"),
            at,
        );
        machine.mount();
        machine.engine.routing_text()
    };
    let first = render(
        &[("zebra", &["z one", "z two"]), ("kilo", &["k one"])],
        &[("bravo", &["b two", "b one"]), ("alpha", &["a one"])],
        "2026-01-01T00:00:00Z",
    );
    let second = render(
        &[("kilo", &["k one"]), ("zebra", &["z one", "z two"])],
        &[("alpha", &["a one"]), ("bravo", &["b two", "b one"])],
        "2026-01-01T03:17:00Z",
    );
    // No time and no note in the block: the stdio text is the same bytes
    // whenever the servers last answered.
    assert_eq!(first, second, "the same bytes both times");
    assert!(!first.contains("Note: "), "{first}");
    assert!(!first.contains("2026-01-01"), "{first}");
    let at = |needle: &str| {
        first
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {first}"))
    };
    let order = [
        at("Route here for local notes questions"),
        at("k one"),
        at("z one"),
        at("a one"),
        at("b two"),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{first}");
    assert!(at("z one") < at("z two") && at("b two") < at("b one"));
}
