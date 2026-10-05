//! The local side of the mount table: a machine with its own domains and
//! connected servers. The engine here is the local daemon's; the servers
//! are real ones on loopback.

use std::sync::Arc;

use crystalline_core::config::{
    DomainEntry, GlobalConfig, OriginConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_remote::{SourceSet, connect_with_token, load_sources};
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{ListDomainsParams, ReadParams};

use crate::fixture::{Options, RemoteServer, engram};

/// A local Crystalline: its own domains (`notes`, and `platform` when
/// `with_platform` is set, a local copy of the servers' team domain), and an
/// empty remote folder of its own.
pub struct LocalMachine {
    pub tmp: tempfile::TempDir,
    pub engine: Arc<Engine>,
}

impl LocalMachine {
    pub async fn start(with_platform: bool) -> LocalMachine {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
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
        let engine = Arc::new(
            Engine::new(
                Arc::new(tokio::sync::Mutex::new(store)),
                cfg,
                None,
                Some(config_path),
            )
            .with_state_dir(root.join("state")),
        );
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
    machine.connect(&server, "acme", "keeper").await;
    let set = machine.mount();
    assert!(set.shadowed().contains("platform"), "{:?}", set.shadowed());
    assert_eq!(set.table().mount("platform").unwrap().source, "acme");
    assert!(
        !local_names(&machine.engine)
            .await
            .contains(&"platform".to_string())
    );
    let refused = machine
        .engine
        .read_engram(
            &ReadParams {
                identifier: "local-note".into(),
                domain: Some("platform".into()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await;
    assert!(
        refused.is_err(),
        "the hidden copy answers nothing, never as a fallback"
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
    assert!(
        report["note"].as_str().unwrap().contains("acme"),
        "{report}"
    );
    assert_eq!(
        machine.engine.sources().unwrap().table().source_of("open"),
        Some("acme"),
        "the mount keeps its name"
    );
    let report = machine.engine.domain_add_virtual("open").await.unwrap();
    assert_eq!(
        report["domain"], "open-local-2",
        "and counts up past its own earlier suffix: {report}"
    );
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
    let refused = machine
        .engine
        .read_engram(
            &ReadParams {
                identifier: "local-note".into(),
                domain: Some("platform".into()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await;
    assert!(
        refused.is_err(),
        "the hidden copy is never an offline fallback"
    );
    assert!(
        !local_names(&machine.engine)
            .await
            .contains(&"platform".to_string())
    );

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
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    let before = machine.engine.routing_text();
    machine.connect(&server, "acme", "keeper").await;
    assert_eq!(
        machine.engine.routing_text(),
        before,
        "no source installed changes nothing"
    );
    machine.mount();
    let text = machine.engine.routing_text();
    let local = text
        .find("Route here for local notes questions")
        .expect("local notes");
    let mounted = text
        .find("Route here for shared questions")
        .expect("acme's open");
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
    assert!(
        report["note"].as_str().unwrap().contains("acme"),
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
