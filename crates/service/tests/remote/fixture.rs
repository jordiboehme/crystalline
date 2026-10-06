//! One served instance with accounts and four domains, for the remote suites.
//!
//! `open` is shared; `lab` is private to `keeper`; `team` reviews changes
//! (every write lands in its author's draft, which a share link opens);
//! `platform` tracks the repository `acme/platform`, so its routing row
//! carries an origin identity. Accounts: `keeper` and `out` are editors,
//! `looker` a viewer, `boss` an admin. The state directory is redirected for
//! the whole process (`ScratchStateDir`), so the maintenance file
//! `hook_status` reads and every file the client side writes stay out of the
//! developer's own folders. A co-editing registry is installed on the engine
//! before the router is built, so a test opens a room in-process and the
//! engine answers reads and edits through it.

// Tasks 6 and 7 use the rest of the fixture (consent, rooms, `stop`).
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, OriginConfig, ResponseFormat, ReviewMode, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::collab::session::{CollabSessions, Joined};
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use serde_json::{Value, json};
use yrs::sync::{Awareness, Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Options as DocOptions, ReadTxn, Text, Transact, Update};

use crate::support::ScratchStateDir;

pub const PASSWORD: &str = "pw12345678";

fn manifest(name: &str, scope: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- {scope}\n\n## When to Use\n\n- Route here for {scope}\n"
    )
}

pub fn engram(title: &str, permalink: &str, line: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n- [fact] {line}\n"
    )
}

/// How the proxy in front of a prefixed instance forwards: `proxy_pass
/// http://daemon/;` strips the prefix, `proxy_pass http://daemon;` passes it
/// through. Both keep `Host`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proxy {
    Strips,
    PassesThrough,
}

/// Which doors the instance has, and the path it is served under.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// `auth.mcp`.
    pub mcp_auth: bool,
    /// `auth.oauth`.
    pub oauth: bool,
    /// The path of `service.public_url`; `None` serves at the root with
    /// `public_url` unset, exactly as before.
    pub prefix: Option<&'static str>,
    /// How the front forwards a prefixed request.
    pub proxy: Proxy,
}

impl Options {
    /// Agents authenticate with personal tokens only.
    pub const TOKENS: Options = Options {
        mcp_auth: true,
        oauth: false,
        prefix: None,
        proxy: Proxy::PassesThrough,
    };
    /// Agents authenticate, and OAuth is served.
    pub const OAUTH: Options = Options {
        mcp_auth: true,
        oauth: true,
        prefix: None,
        proxy: Proxy::PassesThrough,
    };
    /// The legacy open tier.
    pub const OPEN: Options = Options {
        mcp_auth: false,
        oauth: false,
        prefix: None,
        proxy: Proxy::PassesThrough,
    };

    /// The same doors, under `prefix`, behind a front that forwards `proxy`.
    pub const fn under(self, prefix: &'static str, proxy: Proxy) -> Options {
        Options {
            prefix: Some(prefix),
            proxy,
            ..self
        }
    }
}

/// The front a stripping proxy is: `/prefix` and `/prefix/...` reach the
/// daemon without the prefix, every other path (the two host root OAuth
/// documents) reaches it unchanged. Written out here rather than borrowed
/// from the daemon, so a bug in the daemon's own strip cannot hide in the
/// test's.
#[derive(Clone)]
struct StripsPrefix {
    prefix: &'static str,
    inner: axum::Router,
}

impl tower_service::Service<axum::extract::Request> for StripsPrefix {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = <axum::Router as tower_service::Service<axum::extract::Request>>::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        tower_service::Service::<axum::extract::Request>::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, mut request: axum::extract::Request) -> Self::Future {
        let path = request.uri().path().to_string();
        let stripped = if path == self.prefix {
            Some("/".to_string())
        } else {
            path.strip_prefix(&format!("{}/", self.prefix))
                .map(|rest| format!("/{rest}"))
        };
        if let Some(stripped) = stripped {
            let target = match request.uri().query() {
                Some(query) => format!("{stripped}?{query}"),
                None => stripped,
            };
            *request.uri_mut() = target.parse().unwrap();
        }
        self.inner.call(request)
    }
}

/// The serving task and the switch that shuts it down gracefully.
type Serving = (
    tokio::task::JoinHandle<()>,
    tokio::sync::watch::Sender<bool>,
);

/// Serve `router` on `listener` until the returned switch is flipped. Graceful
/// on purpose: `axum::serve` runs every accepted connection as a task of its
/// own, so aborting the serve task alone would leave a pooled keep-alive
/// connection talking to the old router.
fn serve(router: axum::Router, listener: tokio::net::TcpListener) -> Serving {
    let (tx, mut rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = rx.changed().await;
        })
        .await;
    });
    (handle, tx)
}

/// A running instance.
pub struct RemoteServer {
    pub addr: SocketAddr,
    pub tmp: tempfile::TempDir,
    pub auth: Arc<AuthStore>,
    pub engine: Arc<Engine>,
    pub collab: Arc<CollabSessions>,
    pub http: reqwest::Client,
    serving: tokio::sync::Mutex<Option<Serving>>,
    /// The path the instance is served under, empty at the root.
    prefix: &'static str,
    _scratch: ScratchStateDir,
}

impl RemoteServer {
    pub async fn start(options: Options) -> RemoteServer {
        RemoteServer::start_with(options, &[]).await
    }

    /// [`RemoteServer::start`] with extra shared file domains, each holding one
    /// engram `<name>-note` whose fact line names the domain.
    pub async fn start_with(options: Options, extra: &[&str]) -> RemoteServer {
        let scratch = ScratchStateDir::acquire();
        // Bound first: a prefixed instance's `public_url` names this address.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        let mut domains: Vec<(String, String, Option<OriginConfig>, bool)> = vec![
            ("open".into(), "shared questions".into(), None, false),
            (
                "lab".into(),
                "confidential lab questions".into(),
                None,
                false,
            ),
            ("team".into(), "team drafts".into(), None, true),
            (
                "platform".into(),
                "platform questions".into(),
                Some(OriginConfig {
                    repo: "acme/platform".into(),
                    path: None,
                    branch: None,
                    poll_secs: None,
                }),
                false,
            ),
        ];
        for name in extra {
            domains.push((name.to_string(), format!("{name} questions"), None, false));
        }
        for (name, scope, origin, review) in &domains {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("MANIFEST.md"), manifest(name, scope)).unwrap();
            let mut entry = DomainEntry::file(dir);
            entry.origin = origin.clone();
            if *review {
                entry.review = Some(ReviewMode::Overlay);
            }
            cfg.domains.insert(name.clone(), entry);
        }
        std::fs::write(
            root.join("open").join("open-note.md"),
            engram(
                "Open Note",
                "open-note",
                "the vent driver retries a write three times",
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("lab").join("lab-note.md"),
            engram(
                "Lab Note",
                "lab-note",
                "the vent prototype leaks at three bar",
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("platform").join("deploy.md"),
            engram(
                "Deploy",
                "deploy",
                "the platform deploys on tuesdays; see [[open:Open Note]]",
            ),
        )
        .unwrap();
        for name in extra {
            std::fs::write(
                root.join(name).join(format!("{name}-note.md")),
                engram(
                    &format!("{name} note"),
                    &format!("{name}-note"),
                    &format!("the vent in {name} is blue"),
                ),
            )
            .unwrap();
        }
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            public_url: options
                .prefix
                .map(|prefix| format!("http://{addr}{prefix}")),
            ..ServiceConfig::default()
        });
        cfg.auth = Some(AuthConfig {
            mcp: Some(options.mcp_auth),
            oauth: Some(options.oauth),
            ..AuthConfig::default()
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
            .with_token_store_dir(root.join("tokens"))
            .with_state_dir(root.join("state")),
        );
        engine.sync(None).await.unwrap();
        // Installed before the router is built: the router's own registry
        // then finds the engine's handle taken (a no-op second `set`), and the
        // engine answers reads and edits through this one, which a test can
        // open rooms in.
        let collab = CollabSessions::new(engine.clone());
        engine.set_collab_sessions(&collab);
        let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
        for (name, role) in [
            ("keeper", Role::Editor),
            ("out", Role::Editor),
            ("looker", Role::Viewer),
            ("boss", Role::Admin),
        ] {
            auth.add_user(name, name, None, role, PASSWORD)
                .await
                .unwrap();
        }
        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let router = http_router(
            engine.clone(),
            Arc::new(AtomicUsize::new(0)),
            &[],
            auth.clone(),
            None,
        )
        .unwrap();
        let router = match (options.prefix, options.proxy) {
            (Some(prefix), Proxy::Strips) => axum::Router::new().fallback_service(StripsPrefix {
                prefix,
                inner: router,
            }),
            _ => router,
        };
        RemoteServer {
            addr,
            tmp,
            auth,
            engine,
            collab,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            serving: tokio::sync::Mutex::new(Some(serve(router, listener))),
            prefix: options.prefix.unwrap_or(""),
            _scratch: scratch,
        }
    }

    /// The origin a client connects to.
    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The address a client is given: the origin plus the prefix.
    pub fn base(&self) -> String {
        format!("{}{}", self.origin(), self.prefix)
    }

    /// A live personal MCP token for `account`.
    pub async fn token_for(&self, account: &str) -> String {
        self.auth
            .issue_mcp_token(account, "remote test")
            .await
            .unwrap()
            .token
    }

    /// One ctl request, answered as its status and parsed body (`Null` for a
    /// body that is not JSON).
    pub async fn ctl(&self, token: Option<&str>, body: Value) -> (u16, Value) {
        let mut request = self
            .http
            .post(format!("{}/api/v1/ctl", self.base()))
            .header("content-type", "application/json")
            .body(body.to_string());
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// The person in the browser: open `authorize_url`, sign in as `account`,
    /// press Allow, and answer the location the consent sends the browser to.
    pub async fn consent(&self, account: &str, authorize_url: &str) -> String {
        self.consent_with(account, authorize_url, "allow").await
    }

    /// [`RemoteServer::consent`] with the button the person presses: `allow`
    /// or `deny`.
    pub async fn consent_with(&self, account: &str, authorize_url: &str, decision: &str) -> String {
        let started = self.http.get(authorize_url).send().await.unwrap();
        assert_eq!(
            started.status(),
            302,
            "the authorize leg lands on the consent page"
        );
        let page = started.headers()[reqwest::header::LOCATION]
            .to_str()
            .unwrap()
            .to_string();
        let request = page
            .split_once("request=")
            .map(|(_, rest)| rest.split('&').next().unwrap().to_string())
            .unwrap_or_else(|| panic!("no request id in {page}"));
        let login = self
            .http
            .post(format!("{}/api/v1/auth/login", self.base()))
            .json(&json!({ "name": account, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), 200, "the person signs in");
        let cookies = login
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_string))
            .collect::<Vec<_>>()
            .join("; ");
        let csrf = login.json::<Value>().await.unwrap()["csrf"]
            .as_str()
            .unwrap()
            .to_string();
        let decided = self
            .http
            .post(format!(
                "{}/api/v1/oauth/authorizations/{request}",
                self.base()
            ))
            .header("cookie", cookies)
            .header("x-csrf-token", csrf)
            .json(&json!({ "decision": decision }))
            .send()
            .await
            .unwrap();
        assert_eq!(decided.status(), 200, "a decision answers a location");
        decided.json::<Value>().await.unwrap()["location"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A file under a domain's folder.
    pub fn file(&self, domain: &str, name: &str) -> PathBuf {
        self.tmp.path().join(domain).join(name)
    }

    /// Take the server off the network: every later request is refused at the
    /// TCP level, the way a server that went down is.
    pub async fn stop(&self) {
        if let Some((mut handle, switch)) = self.serving.lock().await.take() {
            let _ = switch.send(true);
            if tokio::time::timeout(std::time::Duration::from_secs(2), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }

    /// A person with `permalink` open in the web editor of `domain`, named
    /// `name` in the strip, who typed `line` at the end and has not saved.
    pub async fn person_typing(
        &self,
        domain: &str,
        permalink: &str,
        name: &str,
        line: &str,
    ) -> Room {
        let joined = self.collab.join(domain, permalink, None).await.unwrap();
        let doc = sync_client(&joined).await;
        let mut awareness = Awareness::new(doc.clone());
        awareness.set_local_state_raw(format!("{{\"user\":{{\"name\":\"{name}\"}}}}"));
        let update = awareness.update().unwrap();
        joined
            .session
            .handle_frame(joined.conn, &Message::Awareness(update).encode_v1())
            .await;
        let text = doc.get_or_insert_text("content");
        let edit = {
            let mut txn = doc.transact_mut();
            let end = text.get_string(&txn).encode_utf16().count() as u32;
            text.insert(&mut txn, end, &format!("{line}\n"));
            txn.encode_update_v1()
        };
        joined
            .session
            .handle_frame(
                joined.conn,
                &Message::Sync(SyncMessage::Update(edit)).encode_v1(),
            )
            .await;
        Room { joined, doc }
    }
}

/// An open room and the person's copy of its document.
pub struct Room {
    pub joined: Joined,
    pub doc: Doc,
}

impl Room {
    /// Who the strip names right now.
    pub async fn present(&self) -> Vec<String> {
        self.joined.session.participants(None).await
    }

    /// The person's copy, pulled up to the room's text.
    pub async fn text(&self) -> String {
        let sv = self.doc.transact().state_vector();
        let replies = self
            .joined
            .session
            .handle_frame(
                self.joined.conn,
                &Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1(),
            )
            .await;
        if let Some(Message::Sync(SyncMessage::SyncStep2(update))) =
            messages_of(&replies[0]).first()
        {
            self.doc
                .transact_mut()
                .apply_update(Update::decode_v1(update).unwrap())
                .unwrap();
        }
        let text = self.doc.get_or_insert_text("content");
        text.get_string(&self.doc.transact())
    }
}

fn messages_of(bytes: &[u8]) -> Vec<Message> {
    let mut decoder = DecoderV1::from(bytes);
    MessageReader::new(&mut decoder)
        .collect::<Result<_, _>>()
        .unwrap()
}

async fn sync_client(joined: &Joined) -> Doc {
    let doc = Doc::with_options(DocOptions {
        offset_kind: yrs::OffsetKind::Utf16,
        ..DocOptions::default()
    });
    let sv = doc.transact().state_vector();
    let replies = joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1(),
        )
        .await;
    let Message::Sync(SyncMessage::SyncStep2(update)) = &messages_of(&replies[0])[0] else {
        panic!("step1 is answered with step2");
    };
    doc.transact_mut()
        .apply_update(Update::decode_v1(update).unwrap())
        .unwrap();
    doc
}

/// The PKCE pair the OAuth tests use (RFC 7636 appendix B).
pub const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
pub const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// One OAuth grant, as a client holds it.
pub struct OauthPair {
    pub client_id: String,
    pub access: String,
    pub refresh: String,
}

fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{}={}", encoded(name), encoded(value)))
        .collect::<Vec<_>>()
        .join("&")
}

impl RemoteServer {
    /// A registration, a consent by `account` and an exchange, by hand: the
    /// grant a signed-in client holds.
    pub async fn oauth_pair(&self, account: &str) -> OauthPair {
        let registered: Value = self
            .http
            .post(format!("{}/api/v1/oauth/register", self.base()))
            .json(&json!({ "client_name": "remote test", "redirect_uris": ["http://127.0.0.1/callback"] }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let client_id = registered["client_id"].as_str().unwrap().to_string();
        let redirect = "http://127.0.0.1:9/callback";
        let authorize = format!(
            "{}/api/v1/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&code_challenge={CHALLENGE}&code_challenge_method=S256&state=s1",
            self.base(),
            encoded(&client_id),
            encoded(redirect),
        );
        let location = self.consent(account, &authorize).await;
        let code = location
            .split(['?', '&'])
            .find_map(|pair| pair.strip_prefix("code="))
            .unwrap()
            .to_string();
        let exchanged: Value = self
            .http
            .post(format!("{}/api/v1/oauth/token", self.base()))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("code_verifier", VERIFIER),
                ("client_id", &client_id),
                ("resource", &self.base()),
            ]))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        OauthPair {
            client_id,
            access: exchanged["access_token"].as_str().unwrap().to_string(),
            refresh: exchanged["refresh_token"].as_str().unwrap().to_string(),
        }
    }

    /// `POST /api/v1/oauth/revoke` with `pairs` as the form.
    pub async fn revoke(&self, pairs: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(format!("{}/api/v1/oauth/revoke", self.origin()))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form(pairs))
            .send()
            .await
            .unwrap()
    }
}
