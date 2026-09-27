//! `GET /api/v1/events`: the stream every tab keeps open. What it carries,
//! whom it carries it to, and how a client that stepped away catches up.
//!
//! Its own binary rather than a module of the `rest` group, for the reason
//! `tests/rest/main.rs` gives for keeping `login_throttle` out: the heartbeat
//! case waits fifteen seconds of wall-clock time, and every other case reads
//! frames under tight timeouts that a busy shared binary would make flaky.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ReviewMode, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router_with_shutdown;
use crystalline_service::params::*;
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use tokio::sync::{Mutex, watch};

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";
const MANIFEST: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n";

struct Fixture {
    addr: std::net::SocketAddr,
    engine: Arc<Engine>,
    auth: Arc<AuthStore>,
    shutdown: watch::Sender<bool>,
    _tmp: tempfile::TempDir,
}

/// Point the base directories at a scratch home, once per process, so no
/// write a test makes (the maintenance state file among them) can reach the
/// developer's real state. The support module's `ScratchStateDir` does this
/// for the grouped binaries; this binary uses none of the shared helpers.
fn scratch_home() {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        // SAFETY: set once, before any server task of this process reads
        // them, and never restored: the process ends with the test.
        unsafe {
            std::env::set_var("HOME", home);
            std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
            std::env::set_var("XDG_STATE_HOME", home.join("state"));
            std::env::set_var("XDG_CACHE_HOME", home.join("cache"));
            std::env::set_var("USERPROFILE", home);
            std::env::set_var("APPDATA", home.join("state"));
            std::env::set_var("LOCALAPPDATA", home.join("local"));
        }
        dir
    });
}

/// `eng` (alpha) and `lab` (secret), an admin `root`, an editor `ada` and a
/// viewer `vera`; `anonymous` and `read_only` as asked.
async fn serve(anonymous: bool, read_only: bool) -> Fixture {
    serve_with(anonymous, read_only, false).await
}

/// [`serve`], with `eng` reviewing changes when `review` is set, so a write
/// lands in the writer's own draft.
async fn serve_with(anonymous: bool, read_only: bool, review: bool) -> Fixture {
    scratch_home();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            anonymous: Some(anonymous),
            ..AuthConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let secret = ALPHA.replace("Alpha", "Secret").replace("alpha", "secret");
    for (name, file, text) in [("eng", "alpha.md", ALPHA), ("lab", "secret.md", &secret)] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), MANIFEST.replace("eng", name)).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
        let mut entry = DomainEntry::file(dir);
        if review && name == "eng" {
            entry.review = Some(ReviewMode::Overlay);
        }
        cfg.domains.insert(name.to_string(), entry);
    }
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        read_only: Some(read_only),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_read_only(read_only)
            .with_token_store_dir(root.join("tokens"))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    auth.add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();
    auth.add_user("ada", "Ada", None, Role::Editor, "adapw")
        .await
        .unwrap();
    auth.add_user("vera", "Vera", None, Role::Viewer, "verapw")
        .await
        .unwrap();
    let (shutdown, rx) = watch::channel(false);
    let router = http_router_with_shutdown(
        engine.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
        rx,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        addr,
        engine,
        auth,
        shutdown,
        _tmp: tmp,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> String {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({ "name": name, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| {
            v.strip_prefix("fluid_session=")
                .map(|rest| rest.split(';').next().unwrap().to_string())
        })
        .expect("a session cookie")
}

/// Open the stream as `cookie` (or nobody), with an optional `Last-Event-ID`.
async fn open(
    addr: std::net::SocketAddr,
    cookie: Option<&str>,
    last: Option<&str>,
) -> reqwest::Response {
    // The `Accept` a browser's `EventSource` sends, so the request is the one
    // the UI dispatch in front of the API sees in production.
    let mut req = client()
        .get(format!("http://{addr}/api/v1/events"))
        .header("accept", "text/event-stream");
    if let Some(cookie) = cookie {
        req = req.header("cookie", format!("fluid_session={cookie}"));
    }
    if let Some(last) = last {
        req = req.header("last-event-id", last);
    }
    req.send().await.unwrap()
}

/// One SSE frame: the `id`, `event` and `data` lines of one blank-line
/// delimited block, comments (`: ping`) as `event = "comment"`.
#[derive(Debug, PartialEq)]
struct Frame {
    id: Option<String>,
    event: String,
    data: String,
}

/// Read frames until `n` arrived or `within` elapsed.
async fn read_frames(resp: &mut reqwest::Response, n: usize, within: Duration) -> Vec<Frame> {
    read_until(resp, within, |frames| frames.len() >= n).await
}

/// Read frames until `done` says so or `within` elapsed.
async fn read_until(
    resp: &mut reqwest::Response,
    within: Duration,
    done: impl Fn(&[Frame]) -> bool,
) -> Vec<Frame> {
    let mut buf = String::new();
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while !done(&frames) {
        let chunk = match tokio::time::timeout_at(deadline, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => bytes,
            _ => break,
        };
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
        while let Some(cut) = buf.find("\n\n") {
            let block = buf[..cut].to_string();
            buf.drain(..cut + 2);
            let mut frame = Frame {
                id: None,
                event: String::new(),
                data: String::new(),
            };
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix(':') {
                    frame.event = "comment".to_string();
                    frame.data = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("id:") {
                    frame.id = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("event:") {
                    frame.event = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    frame.data.push_str(rest.trim());
                }
            }
            frames.push(frame);
        }
    }
    frames
}

fn edit(domain: &str, identifier: &str, line: &str) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        domain: domain.to_string(),
        operation: "append".to_string(),
        content: Some(line.to_string()),
        ..EditParams::default()
    }
}

/// The `domain` of every frame, in order.
fn domains_of(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .map(|f| {
            serde_json::from_str::<serde_json::Value>(&f.data).unwrap()["domain"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// The sequence number the next announcement will carry: the fixture's
/// start-up sync may already have announced what it indexed.
fn next_seq(engine: &Engine) -> u64 {
    engine.changes().last_id().map_or(0, |id| id.seq) + 1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_carries_an_engram_frame_with_an_id_and_the_right_headers() {
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let mut resp = open(fx.addr, Some(&cookie), None).await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(resp.headers()["cache-control"], "no-cache");
    assert_eq!(resp.headers()["x-accel-buffering"], "no");
    let seq = next_seq(&fx.engine);
    fx.engine
        .edit_engram(&edit("eng", "alpha", "a line"))
        .await
        .unwrap();
    let frames = read_frames(&mut resp, 1, Duration::from_secs(5)).await;
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].event, "engram");
    let id = frames[0].id.as_deref().expect("an id");
    assert_eq!(id, format!("{}:{}", fx.engine.changes().epoch(), seq));
    let data: serde_json::Value = serde_json::from_str(&frames[0].data).unwrap();
    assert_eq!(data["domain"], "eng");
    assert_eq!(data["permalink"], "alpha");
    assert_eq!(data["kind"], "modified");
    assert!(data["checksum"].is_string());
    assert!(data.get("event").is_none(), "the tag rides the event line");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quiet_stream_sends_a_ping_comment_as_its_heartbeat() {
    // Real time rather than `tokio::time::pause`: the keep-alive timer lives
    // inside axum's stream on the server task and a paused clock on the
    // test's runtime would not move it. Fifteen seconds once, in this one
    // test, is the price of asserting the heartbeat over the wire.
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let mut resp = open(fx.addr, Some(&cookie), None).await;
    let frames = read_frames(&mut resp, 1, Duration::from_secs(20)).await;
    assert_eq!(
        frames,
        vec![Frame {
            id: None,
            event: "comment".to_string(),
            data: "ping".to_string()
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_outside_a_private_domain_never_hears_it_and_the_admin_does() {
    let fx = serve(false, false).await;
    fx.auth
        .set_domain_visibility("lab", true, "root")
        .await
        .unwrap();
    let ada = login(fx.addr, "ada", "adapw").await;
    let root = login(fx.addr, "root", "rootpw").await;
    let mut outsider = open(fx.addr, Some(&ada), None).await;
    let mut admin = open(fx.addr, Some(&root), None).await;
    fx.engine
        .edit_engram(&edit("lab", "secret", "a line"))
        .await
        .unwrap();
    fx.engine
        .edit_engram(&edit("eng", "alpha", "a line"))
        .await
        .unwrap();
    let heard = read_frames(&mut outsider, 2, Duration::from_secs(3)).await;
    assert_eq!(
        domains_of(&heard),
        vec!["eng"],
        "the private domain is silent for an outsider: {heard:?}"
    );
    let heard = read_frames(&mut admin, 2, Duration::from_secs(3)).await;
    assert_eq!(heard.len(), 2, "the admin hears both: {heard:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_private_domains_rename_and_removal_are_silent_to_an_outsider_and_heard_by_a_member() {
    // Section J (k), ruled 2026-09-27: the old name leaves the privacy
    // records with the change itself, so the engine resolves its audience
    // eagerly, at the moment of the change, and that snapshot is what the
    // stream checks, never a session's cache and never the registry's
    // state after; nothing leaks to the outsider, nothing is held back
    // from the member. The new name's frame rides the ordinary check, which
    // a captured audience forces to re-resolve, so the outsider does not
    // hear the new name from a cache that predates it either.
    let fx = serve(false, false).await;
    fx.auth
        .set_domain_visibility("lab", true, "root")
        .await
        .unwrap();
    let ada = login(fx.addr, "ada", "adapw").await;
    let root = login(fx.addr, "root", "rootpw").await;
    let mut outsider = open(fx.addr, Some(&ada), None).await;
    let mut member = open(fx.addr, Some(&root), None).await;
    fx.engine
        .rename_domain(
            "lab",
            "vault",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    fx.engine
        .unregister_domain(
            "vault",
            &crystalline_service::Scope::Unrestricted,
            false,
            &[],
        )
        .await
        .unwrap();
    let heard = read_frames(&mut outsider, 1, Duration::from_secs(2)).await;
    assert!(
        heard.is_empty(),
        "a private domain's rename and its removal reach an outsider not at all: {heard:?}"
    );
    let heard = read_frames(&mut member, 3, Duration::from_secs(3)).await;
    let names = domains_of(&heard);
    assert!(
        names.contains(&"lab".to_string()) && names.contains(&"vault".to_string()),
        "a member hears the rename and the removal: {heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_members_revoked_access_is_captured_before_a_rename_so_they_do_not_hear_it() {
    // Section J (k), ruled 2026-09-27: the audience is what the engine
    // resolves eagerly at the moment of the change, never a session's own
    // cache, which is still within VISIBILITY_TTL two seconds after a
    // revocation and would otherwise still answer "member".
    let fx = serve(false, false).await;
    fx.auth
        .set_domain_visibility("lab", true, "root")
        .await
        .unwrap();
    fx.auth
        .upsert_domain_member("lab", "ada", MemberLevel::Manager, "root")
        .await
        .unwrap();
    let ada = login(fx.addr, "ada", "adapw").await;
    let mut revoked = open(fx.addr, Some(&ada), None).await;
    fx.auth.remove_domain_member("lab", "ada").await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    fx.engine
        .rename_domain(
            "lab",
            "vault",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = read_frames(&mut revoked, 1, Duration::from_secs(2)).await;
    assert!(
        heard.is_empty(),
        "revoked two seconds before the rename: the engine's own eager read of the member list excludes ada, whatever ada's session still has cached: {heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_draft_frame_reaches_its_author_and_nobody_else() {
    let fx = serve_with(false, false, true).await;
    let ada = login(fx.addr, "ada", "adapw").await;
    let vera = login(fx.addr, "vera", "verapw").await;
    let mut author = open(fx.addr, Some(&ada), None).await;
    let mut other = open(fx.addr, Some(&vera), None).await;
    fx.engine
        .edit_engram_as(
            &edit("eng", "alpha", "ada's draft line"),
            None,
            &crystalline_service::Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    let heard = read_frames(&mut author, 1, Duration::from_secs(3)).await;
    assert_eq!(heard.len(), 1, "{heard:?}");
    let data: serde_json::Value = serde_json::from_str(&heard[0].data).unwrap();
    assert_eq!(data["draft_of"], "ada");
    let heard = read_frames(&mut other, 1, Duration::from_secs(2)).await;
    assert!(heard.is_empty(), "not even as an invalidation: {heard:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_last_event_id_replays_what_the_ring_holds_after_it() {
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let epoch = fx.engine.changes().epoch();
    let first = next_seq(&fx.engine);
    fx.engine
        .edit_engram(&edit("eng", "alpha", "one"))
        .await
        .unwrap();
    fx.engine
        .edit_engram(&edit("eng", "alpha", "two"))
        .await
        .unwrap();
    fx.engine
        .edit_engram(&edit("eng", "alpha", "three"))
        .await
        .unwrap();
    let mut resp = open(fx.addr, Some(&cookie), Some(&format!("{epoch}:{first}"))).await;
    let frames = read_frames(&mut resp, 2, Duration::from_secs(3)).await;
    let ids: Vec<&str> = frames.iter().filter_map(|f| f.id.as_deref()).collect();
    assert_eq!(
        ids,
        vec![
            format!("{epoch}:{}", first + 1),
            format!("{epoch}:{}", first + 2)
        ]
    );
    // The live stream continues behind the replay.
    fx.engine
        .edit_engram(&edit("eng", "alpha", "four"))
        .await
        .unwrap();
    let more = read_frames(&mut resp, 1, Duration::from_secs(3)).await;
    assert_eq!(
        more[0].id.as_deref(),
        Some(format!("{epoch}:{}", first + 3).as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreign_epoch_a_dropped_id_and_a_lagged_subscriber_each_get_a_reset() {
    use crystalline_service::changes::{CHANNEL_CAPACITY, Change, DomainChanged, RING_CAPACITY};

    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let epoch = fx.engine.changes().epoch();
    let mut resp = open(fx.addr, Some(&cookie), Some(&format!("{}:1", epoch + 7))).await;
    let frames = read_frames(&mut resp, 1, Duration::from_secs(3)).await;
    assert_eq!(frames[0].event, "reset");
    assert_eq!(frames[0].id, None, "a reset is outside the sequence");

    let before = next_seq(&fx.engine);
    for i in 0..(RING_CAPACITY + 5) {
        fx.engine
            .edit_engram(&edit("eng", "alpha", &format!("line {i}")))
            .await
            .unwrap();
    }
    let mut resp = open(
        fx.addr,
        Some(&cookie),
        Some(&format!("{epoch}:{}", before + 1)),
    )
    .await;
    let frames = read_frames(&mut resp, 1, Duration::from_secs(3)).await;
    assert_eq!(
        frames[0].event, "reset",
        "the entry after the id fell out of the ring"
    );

    // Lag: park a subscriber, overfill the channel behind it, then read. A
    // parked client alone does not stop the server from draining the channel
    // into the socket's buffers, so each frame is made large (a long domain
    // name): the buffers fill after a few frames, the server's write waits,
    // and the channel overflows behind it. The frames written before that are
    // read first, so the reset is looked for among all of them rather than
    // at the head.
    let mut parked = open(fx.addr, Some(&cookie), None).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let padding = "x".repeat(16 * 1024);
    for i in 0..(CHANNEL_CAPACITY + 50) {
        fx.engine.changes().announce(Change::Domain(DomainChanged {
            domain: format!("d{i}{padding}"),
            actor: None,
            audience: None,
        }));
    }
    let frames = read_until(&mut parked, Duration::from_secs(10), |frames| {
        frames.iter().any(|f| f.event == "reset")
    })
    .await;
    assert!(
        frames.iter().any(|f| f.event == "reset"),
        "a lagged subscriber is told to refetch everything ({} frames, none a reset)",
        frames.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_garbage_or_forged_last_event_id_gets_a_reset_and_the_stream_goes_on() {
    // Catches a malformed header treated as "no header" (the client would
    // miss what happened while it was away without being told) and a forged
    // sequence number handed to the ring, where `u64::MAX` overflows.
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let epoch = fx.engine.changes().epoch();
    for last in [
        "garbage".to_string(),
        format!("{epoch}:{}", u64::MAX),
        format!("{epoch}:{}", next_seq(&fx.engine) + 5),
    ] {
        let mut resp = open(fx.addr, Some(&cookie), Some(&last)).await;
        assert_eq!(resp.status(), 200, "{last}");
        let frames = read_frames(&mut resp, 1, Duration::from_secs(3)).await;
        assert_eq!(frames.len(), 1, "{last}: {frames:?}");
        assert_eq!(frames[0].event, "reset", "{last}");
        fx.engine
            .edit_engram(&edit("eng", "alpha", &format!("after {last}")))
            .await
            .unwrap();
        let more = read_frames(&mut resp, 1, Duration::from_secs(3)).await;
        assert_eq!(more.len(), 1, "{last}: the stream goes on after the reset");
        assert_eq!(more[0].event, "engram", "{last}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_identity_is_401_the_anonymous_viewer_is_200_and_read_only_serves_it() {
    let closed = serve(false, false).await;
    let resp = open(closed.addr, None, None).await;
    assert_eq!(resp.status(), 401);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");

    let published = serve(true, true).await;
    // `lab` is private, so the anonymous viewer's stream is filtered like
    // any anonymous read (R1, P20; review I3).
    published
        .auth
        .set_domain_visibility("lab", true, "root")
        .await
        .unwrap();
    let mut resp = open(published.addr, None, None).await;
    assert_eq!(
        resp.status(),
        200,
        "the anonymous viewer subscribes at viewer level on a read-only instance"
    );
    // What a read-only instance still announces: the watcher's own finds.
    std::fs::write(
        published._tmp.path().join("eng/alpha.md"),
        ALPHA.replace("A rule", "A revised rule"),
    )
    .unwrap();
    std::fs::write(
        published._tmp.path().join("lab/secret.md"),
        ALPHA
            .replace("Alpha", "Secret")
            .replace("alpha", "secret")
            .replace("A rule", "A revised rule"),
    )
    .unwrap();
    published
        .engine
        .sync_paths("lab", vec!["secret.md".to_string()])
        .await
        .unwrap();
    published
        .engine
        .sync_paths("eng", vec!["alpha.md".to_string()])
        .await
        .unwrap();
    let frames = read_frames(&mut resp, 2, Duration::from_secs(3)).await;
    assert_eq!(
        domains_of(&frames),
        vec!["eng"],
        "the private domain is silent: {frames:?}"
    );
    assert_eq!(frames[0].event, "engram");
    let data: serde_json::Value = serde_json::from_str(&frames[0].data).unwrap();
    assert_eq!(data["actor"], serde_json::Value::Null);
}

/// Past the ten seconds a stream trusts what it resolved.
const PAST_TTL: Duration = Duration::from_secs(11);

/// Read until the body ends or `within` elapsed: the frames seen, and
/// whether it ended.
async fn read_to_end(resp: &mut reqwest::Response, within: Duration) -> (Vec<String>, bool) {
    let mut seen = Vec::new();
    let ended = tokio::time::timeout(within, async {
        loop {
            match resp.chunk().await {
                Ok(Some(bytes)) => seen.push(String::from_utf8_lossy(&bytes).to_string()),
                Ok(None) | Err(_) => return,
            }
        }
    })
    .await
    .is_ok();
    (seen, ended)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_session_ends_its_stream_at_the_next_refresh() {
    // Catches the identity frozen at connect (review I1, ruled): the session
    // is revoked (what a logout does to it), and the first event after the
    // refresh is due ends the stream instead of being written.
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let mut resp = open(fx.addr, Some(&cookie), None).await;
    fx.auth.delete_session(&cookie).await.unwrap();
    tokio::time::sleep(PAST_TTL).await;
    fx.engine
        .edit_engram(&edit("eng", "alpha", "after the logout"))
        .await
        .unwrap();
    let (seen, ended) = read_to_end(&mut resp, Duration::from_secs(3)).await;
    assert!(ended, "the stream ends");
    assert!(
        !seen.iter().any(|chunk| chunk.contains("event: engram")),
        "nothing is written to a session that is gone: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disabled_account_ends_its_stream_at_the_next_refresh() {
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let mut resp = open(fx.addr, Some(&cookie), None).await;
    fx.auth.set_disabled("ada", true).await.unwrap();
    tokio::time::sleep(PAST_TTL).await;
    fx.engine
        .edit_engram(&edit("eng", "alpha", "after the disable"))
        .await
        .unwrap();
    let (seen, ended) = read_to_end(&mut resp, Duration::from_secs(3)).await;
    assert!(ended, "the stream ends");
    assert!(
        !seen.iter().any(|chunk| chunk.contains("event: engram")),
        "nothing is written to a disabled account: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_demoted_admin_loses_admin_visibility_at_the_next_refresh() {
    // Catches the connect-time `admin` flag kept for the life of the stream
    // (review I1, ruled): `chief` hears the private `lab` as an admin, is
    // demoted, and after the refresh hears `eng` only.
    let fx = serve(false, false).await;
    fx.auth
        .add_user("chief", "Chief", None, Role::Admin, "chiefpw")
        .await
        .unwrap();
    fx.auth
        .set_domain_visibility("lab", true, "root")
        .await
        .unwrap();
    let cookie = login(fx.addr, "chief", "chiefpw").await;
    let mut resp = open(fx.addr, Some(&cookie), None).await;
    fx.engine
        .edit_engram(&edit("lab", "secret", "while an admin"))
        .await
        .unwrap();
    let heard = read_frames(&mut resp, 1, Duration::from_secs(3)).await;
    assert_eq!(
        domains_of(&heard),
        vec!["lab"],
        "an admin hears the private domain"
    );
    fx.auth.set_role("chief", Role::Viewer).await.unwrap();
    tokio::time::sleep(PAST_TTL).await;
    fx.engine
        .edit_engram(&edit("lab", "secret", "after the demotion"))
        .await
        .unwrap();
    fx.engine
        .edit_engram(&edit("eng", "alpha", "after the demotion"))
        .await
        .unwrap();
    let heard = read_frames(&mut resp, 2, Duration::from_secs(3)).await;
    assert_eq!(domains_of(&heard), vec!["eng"], "{heard:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_member_hears_a_private_domain_and_its_rename() {
    // Review M5: the `Accounts` path end to end for a member who is neither
    // the owner nor an admin.
    let fx = serve(false, false).await;
    fx.auth
        .set_domain_visibility("lab", true, "root")
        .await
        .unwrap();
    fx.auth
        .upsert_domain_member("lab", "vera", MemberLevel::Viewer, "root")
        .await
        .unwrap();
    let vera = login(fx.addr, "vera", "verapw").await;
    let mut member = open(fx.addr, Some(&vera), None).await;
    fx.engine
        .edit_engram(&edit("lab", "secret", "a line"))
        .await
        .unwrap();
    fx.engine
        .rename_domain(
            "lab",
            "vault",
            true,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = read_until(&mut member, Duration::from_secs(3), |frames| {
        domains_of(frames).contains(&"vault".to_string())
    })
    .await;
    let names = domains_of(&heard);
    assert_eq!(names.first().map(String::as_str), Some("lab"), "{heard:?}");
    assert!(
        names.contains(&"vault".to_string()),
        "the member hears the rename's new name: {heard:?}"
    );
    assert!(
        heard
            .iter()
            .any(|f| f.event == "domain" && domains_of(std::slice::from_ref(f)) == ["lab"]),
        "and its old name's domain frame: {heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_account_holds_at_most_thirty_two_streams() {
    // Review M3, ruled: the thirty-third stream of one account is refused
    // 503 with `Retry-After`, while another account still subscribes.
    let fx = serve(false, false).await;
    let ada = login(fx.addr, "ada", "adapw").await;
    let mut open_streams = Vec::new();
    for _ in 0..32 {
        let resp = open(fx.addr, Some(&ada), None).await;
        assert_eq!(resp.status(), 200);
        open_streams.push(resp);
    }
    let refused = open(fx.addr, Some(&ada), None).await;
    assert_eq!(refused.status(), 503);
    assert_eq!(refused.headers()["retry-after"], "30");
    assert_eq!(
        refused.headers()["content-type"],
        "application/problem+json"
    );
    let vera = login(fx.addr, "vera", "verapw").await;
    assert_eq!(open(fx.addr, Some(&vera), None).await.status(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_ends_when_the_shutdown_watch_flips() {
    let fx = serve(false, false).await;
    let cookie = login(fx.addr, "ada", "adapw").await;
    let mut resp = open(fx.addr, Some(&cookie), None).await;
    fx.shutdown.send(true).unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match resp.chunk().await {
                Ok(Some(_)) => continue,
                Ok(None) => break true,
                Err(_) => break true,
            }
        }
    })
    .await;
    assert_eq!(ended, Ok(true), "the body closes on shutdown");
}
