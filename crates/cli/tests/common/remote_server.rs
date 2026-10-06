//! A Crystalline server inside the test process, for the CLI's source tests:
//! the binary under test runs as a child, isolated per test, and connects to
//! this server over loopback.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_service::rest::{AuthStore, Role};

fn manifest(name: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- The shared {name} domain\n\n## When to Use\n\n- Route here for shared {name} questions\n"
    )
}

fn note(name: &str, permalink: &str, line: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {name}\npermalink: {permalink}\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n- [fact] {line}\n"
    )
}

pub struct CliServer {
    pub origin: String,
    pub auth: Arc<AuthStore>,
    pub tmp: tempfile::TempDir,
    serve: tokio::task::JoinHandle<()>,
    runtime: tokio::runtime::Runtime,
}

impl CliServer {
    pub fn start() -> CliServer {
        CliServer::start_with(&[])
    }

    pub fn start_with(extra: &[&str]) -> CliServer {
        CliServer::start_full(extra, &[])
    }

    /// [`CliServer::start_with`] plus one shared domain per `(name, repo)`
    /// that tracks that GitHub repository, so a local domain tracking the
    /// same one is the same domain.
    pub fn start_full(extra: &[&str], tracked: &[(&str, &str)]) -> CliServer {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("server-home");
        // SAFETY: one process per test under nextest, and this runs before
        // the runtime or any child exists. The server keeps its maintenance
        // record under this process's state directory.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_STATE_HOME", home.join("state"));
            std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
            std::env::set_var("XDG_CACHE_HOME", home.join("cache"));
            std::env::set_var("USERPROFILE", &home);
            std::env::set_var("APPDATA", home.join("roaming"));
            std::env::set_var("LOCALAPPDATA", home.join("local"));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let root = tmp.path().to_path_buf();
        let mut extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        extra.extend(tracked.iter().map(|(name, _)| name.to_string()));
        let tracked: Vec<(String, String)> = tracked
            .iter()
            .map(|(n, r)| (n.to_string(), r.to_string()))
            .collect();
        let (origin, auth, serve) = runtime.block_on(async move {
            let mut cfg = GlobalConfig::default();
            let mut names = vec!["open".to_string()];
            names.extend(extra);
            for name in &names {
                let dir = root.join(name);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("MANIFEST.md"), manifest(name)).unwrap();
                let mut entry = DomainEntry::file(dir);
                if let Some((_, repo)) = tracked.iter().find(|(n, _)| n == name) {
                    entry.origin = Some(crystalline_core::config::OriginConfig {
                        repo: repo.clone(),
                        path: None,
                        branch: None,
                        poll_secs: None,
                    });
                }
                cfg.domains.insert(name.clone(), entry);
            }
            std::fs::write(
                root.join("open").join("vent-driver.md"),
                note(
                    "Vent Driver",
                    "vent-driver",
                    "the vent driver retries a write three times before it gives up",
                ),
            )
            .unwrap();
            for name in names.iter().skip(1) {
                std::fs::write(
                    root.join(name).join(format!("{name}-note.md")),
                    note(
                        &format!("{name} note"),
                        &format!("{name}-note"),
                        &format!("the vent in {name} is blue"),
                    ),
                )
                .unwrap();
            }
            cfg.service = Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                ..ServiceConfig::default()
            });
            cfg.auth = Some(AuthConfig {
                mcp: Some(true),
                oauth: Some(false),
                ..AuthConfig::default()
            });
            let config_path = root.join("config.yaml");
            crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
            let store = crystalline_index::TursoStore::open_in_memory()
                .await
                .unwrap();
            let engine = Arc::new(
                crystalline_service::Engine::new(
                    Arc::new(tokio::sync::Mutex::new(store)),
                    cfg,
                    None,
                    Some(config_path),
                )
                .with_token_store_dir(root.join("tokens")),
            );
            engine.sync(None).await.unwrap();
            let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
            auth.add_user("keeper", "keeper", None, Role::Editor, "pw12345678")
                .await
                .unwrap();
            let router = crystalline_service::daemon::http_router(
                engine,
                Arc::new(AtomicUsize::new(0)),
                &[],
                auth.clone(),
                None,
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let serve = tokio::spawn(async move {
                let _ = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await;
            });
            (origin, auth, serve)
        });
        CliServer {
            origin,
            auth,
            tmp,
            serve,
            runtime,
        }
    }

    /// A live personal MCP token for `account`.
    pub fn token_for(&self, account: &str) -> String {
        self.runtime
            .block_on(self.auth.issue_mcp_token(account, "cli test"))
            .unwrap()
            .token
    }

    /// The token of a new token-only account (no password) named `name`, an
    /// editor: what an agent connects with.
    pub fn agent_token(&self, name: &str) -> String {
        self.runtime
            .block_on(
                self.auth
                    .add_token_only_user(name, name, None, Role::Editor, "agent"),
            )
            .unwrap()
            .token
    }

    /// Put a maintenance record where this server reads it.
    pub fn write_maintenance(&self, state: serde_json::Value) {
        let path = crystalline_service::maintenance::path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    }

    /// Take the server off the network.
    pub fn stop(&self) {
        self.serve.abort();
        std::thread::sleep(Duration::from_millis(200));
    }
}
