//! The trigger webhooks: a local listener records what the daemon's trigger
//! task posts, and a paused clock steps its windows, so a minute or half an
//! hour is virtual time that passes only once the runtime is idle.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use crystalline_core::automation::{Loop, RunOutcome, RunReport, webhook_signature};
use crystalline_core::config::{AutomationConfig, GlobalConfig, WebhookConfig};
use crystalline_service::Scope;
use crystalline_service::automation_trigger::{
    HttpPoster, Poster, RATE_LIMIT, WebhookRequest, run_automation_triggers_with, webhook_request,
};
use crystalline_service::params::{DiscussParams, WriteReflectionParams};
use crystalline_service::{Engine, ReflectionCaller};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::fixture::{Fx, engram, fixture_with, manifest};

const SECRET: &str = "fake-secret";

const PAGE: &str = "# D\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night.\n\n## Day deploys\n<!-- r:bbbb2222 -->\n\nSome run by day.\n";

/// One webhook the trigger task posted.
struct Hit {
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    at: Instant,
}

impl Hit {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A [`Poster`] that records every webhook and answers with `status`. The
/// paused clock jumps across real socket waits, so the timing tests post
/// here; `the_wire_carries_the_signed_request` covers the HTTP side.
struct Recorder {
    tx: mpsc::UnboundedSender<Hit>,
    status: Arc<AtomicU16>,
    /// The next post panics before it records anything.
    panic_next: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Poster for Recorder {
    async fn post(&self, request: WebhookRequest) -> Result<(), String> {
        if self.panic_next.swap(false, Ordering::SeqCst) {
            panic!("the poster broke");
        }
        let _ = self.tx.send(Hit {
            headers: request.headers,
            body: request.body,
            at: Instant::now(),
        });
        match self.status.load(Ordering::SeqCst) {
            code if (200..300).contains(&code) => Ok(()),
            code => Err(format!("the receiver answered {code}")),
        }
    }
}

/// The receiver: what the task posted, and what it answers.
struct Hook {
    url: String,
    hits: mpsc::UnboundedReceiver<Hit>,
    status: Arc<AtomicU16>,
    panic_next: Arc<AtomicBool>,
    poster: Arc<Recorder>,
}

fn hook() -> Hook {
    let (tx, hits) = mpsc::unbounded_channel();
    let status = Arc::new(AtomicU16::new(204));
    let panic_next = Arc::new(AtomicBool::new(false));
    Hook {
        url: "https://hooks.example/webhook/abc".to_string(),
        hits,
        status: status.clone(),
        panic_next: panic_next.clone(),
        poster: Arc::new(Recorder {
            tx,
            status,
            panic_next,
        }),
    }
}

impl Hook {
    /// The next webhook inside `within`, or `None`.
    async fn next(&mut self, within: Duration) -> Option<Hit> {
        tokio::time::timeout(within, self.hits.recv())
            .await
            .ok()
            .flatten()
    }
}

/// The automation block with `url` (when given) and the secret.
fn webhook(cfg: &mut GlobalConfig, url: Option<&str>, threshold: Option<u64>) {
    cfg.automation = Some(AutomationConfig {
        webhook: url.map(|url| WebhookConfig {
            url: Some(url.to_string()),
            secret: Some(SECRET.to_string()),
        }),
        evolve_threshold: threshold,
    });
}

fn person() -> ReflectionCaller {
    ReflectionCaller::person(Scope::Unrestricted, "jordi")
}

fn agent() -> ReflectionCaller {
    ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false)
}

/// `eng` with Reflection on, `extra` MANIFEST lines, the webhook at `url`
/// and the page `d.md` written by the agent.
async fn reflecting(extra: &str, url: Option<&str>) -> Fx {
    let fx = fixture_with(
        &[(
            "MANIFEST.md",
            &manifest(&format!("reflection: on\n{extra}")),
        )],
        |cfg, _| webhook(cfg, url, None),
    )
    .await;
    fx.engine.set_automation_now(Some(
        crystalline_core::time::parse_rfc3339("2026-10-21T10:00:00+00:00").unwrap(),
    ));
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "d.md".into(),
                markdown: Some(PAGE.into()),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    fx
}

async fn open_thread(engine: &Engine, text: &str) -> String {
    let opened = engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("d.md".into()),
                section: Some("aaaa1111".into()),
                text: Some(text.into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    opened["id"].as_str().unwrap().to_string()
}

/// The trigger task, running, with its stop switch.
struct Running {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

async fn start(engine: &Arc<Engine>, hook: &Hook) -> Running {
    let (stop, rx) = watch::channel(false);
    let handle = tokio::spawn(run_automation_triggers_with(
        engine.clone(),
        rx,
        hook.poster.clone(),
    ));
    // Let it subscribe to the bus before the test changes anything.
    tokio::task::yield_now().await;
    Running { stop, handle }
}

impl Running {
    async fn stop(self) {
        self.stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.handle)
            .await
            .expect("the task stops on shutdown")
            .unwrap();
    }
}

async fn take(engine: &Engine) -> String {
    let taken = engine
        .automation_lease_take(Loop::Reflect, "eng", "n8n", &Scope::Unrestricted)
        .await
        .unwrap();
    taken["token"].as_str().unwrap().to_string()
}

/// The delivery state of `eng` once the sender recorded an outcome.
async fn recorded(engine: &Engine, lp: Loop) -> crystalline_service::engine::Delivery {
    for _ in 0..100 {
        let delivery = engine.automation_delivery("eng", lp);
        if delivery.last_sent.is_some() || delivery.last_error.is_some() {
            return delivery;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no delivery was recorded");
}

fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

#[test]
fn a_known_vector_signs_as_expected() {
    // printf '%s' '1761134400.{"event":"reflect","domain":"eng"}' \
    //   | openssl dgst -sha256 -hmac "fake-secret"
    assert_eq!(
        webhook_signature(
            b"fake-secret",
            "1761134400",
            br#"{"event":"reflect","domain":"eng"}"#
        ),
        "sha256=4fe278d8d63d1448b9f568384d7ae7cf14a266f68e4cbea496f927dfa4def3f7"
    );
}

#[tokio::test(start_paused = true)]
async fn a_new_thread_sends_one_signed_webhook_within_a_minute() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;

    let opened = Instant::now();
    open_thread(&fx.engine, "Still true?").await;
    let hit = hook.next(minutes(2)).await.expect("one webhook");
    assert!(
        hit.at - opened <= Duration::from_secs(60),
        "sent {:?} after the thread",
        hit.at - opened
    );

    assert_eq!(hit.header("x-crystalline-event"), "reflect");
    assert_eq!(hit.header("content-type"), "application/json");
    let timestamp = hit.header("x-crystalline-timestamp");
    let seconds: i64 = timestamp.parse().expect("unix seconds");
    assert!((crystalline_core::time::now().unix_seconds() - seconds).abs() < 120);
    assert_eq!(
        hit.header("x-crystalline-signature"),
        webhook_signature(SECRET.as_bytes(), timestamp, &hit.body)
    );
    let body = hit.json();
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["counts", "domain", "event", "instance", "sent_at"],
        "{body}"
    );
    assert_eq!(body["event"], "reflect");
    assert_eq!(body["domain"], "eng");
    assert!(body["instance"].is_string(), "{body}");
    assert!(
        crystalline_core::time::parse_rfc3339(body["sent_at"].as_str().unwrap()).is_some(),
        "{body}"
    );
    assert_eq!(body["counts"]["waiting_for_the_agent"], 1, "{body}");
    // Counts only: no text and no path of the domain.
    let text = String::from_utf8(hit.body.to_vec()).unwrap();
    assert!(
        !text.contains("Still true") && !text.contains("d.md"),
        "{text}"
    );

    // The receiver saw it before its answer reached the sender.
    let delivery = recorded(&fx.engine, Loop::Reflect).await;
    assert!(delivery.configured);
    assert!(delivery.last_sent.is_some() && delivery.last_error.is_none());
    assert!(hook.next(minutes(10)).await.is_none(), "one, not more");
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn ten_threads_in_a_minute_send_one() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;

    for n in 0..10 {
        open_thread(&fx.engine, &format!("Question {n}?")).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let hit = hook.next(minutes(2)).await.expect("one webhook");
    assert_eq!(hit.json()["counts"]["waiting_for_the_agent"], 10);
    assert!(hook.next(minutes(10)).await.is_none(), "one for all ten");
    task.stop().await;
}

/// A page whose section draws on `runbooks/deploy`, then a change to that
/// engram: one update waits.
async fn with_a_source_update(fx: &Fx) -> std::path::PathBuf {
    let source = fx.root.join("runbooks/deploy.md");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(
        &source,
        engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "d.md".into(),
                sources: BTreeMap::from([(
                    "aaaa1111".to_string(),
                    vec!["runbooks/deploy".to_string()],
                )]),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    change_source(fx, &source, "in the morning").await;
    source
}

async fn change_source(fx: &Fx, source: &std::path::Path, when: &str) {
    std::fs::write(
        source,
        engram(
            "Deploy",
            "runbooks/deploy",
            &format!("- [fact] deploys run {when}"),
        ),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn source_changes_wait_for_thirty_quiet_minutes() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    let begin = Instant::now();
    let source = with_a_source_update(&fx).await;
    let status = fx
        .engine
        .automation_status(Loop::Reflect, &["eng".to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["domains"][0]["counts"]["updates"], 1, "{status}");

    // Ten minutes on, the source changes again: the half hour starts over.
    tokio::time::sleep(minutes(10)).await;
    change_source(&fx, &source, "at noon").await;
    assert!(
        hook.next(minutes(29)).await.is_none(),
        "nothing while the sources are not quiet for half an hour"
    );
    let hit = hook.next(minutes(3)).await.expect("one webhook");
    let after = hit.at - begin;
    assert!(
        after >= minutes(40) && after <= minutes(42),
        "sent {after:?} after the start"
    );
    let body = hit.json();
    assert_eq!(body["counts"]["updates"], 1, "{body}");
    assert_eq!(body["counts"]["waiting_for_the_agent"], 0, "{body}");
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_while_the_lease_is_held_and_one_is_sent_on_release() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    let token = take(&fx.engine).await;
    open_thread(&fx.engine, "Still true?").await;
    assert!(hook.next(minutes(5)).await.is_none(), "not while it runs");

    let released = fx
        .engine
        .automation_lease_release(Loop::Reflect, "eng", &token, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(released["released"], true);
    let gone = Instant::now();
    let hit = hook.next(minutes(2)).await.expect("one on release");
    assert!(
        hit.at - gone <= Duration::from_secs(61),
        "{:?}",
        hit.at - gone
    );
    assert_eq!(hit.json()["counts"]["waiting_for_the_agent"], 1);
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_runs_own_writes_send_nothing() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    // A person asks; a runner takes the lease inside the window.
    let thread = open_thread(&fx.engine, "Still true?").await;
    let token = take(&fx.engine).await;
    // The run answers the thread (it now waits for the person) and rewrites
    // the page, as the agent.
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("answer".into()),
                path: Some("d.md".into()),
                thread: Some(thread),
                text: Some("Yes, still at night.".into()),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(
            &WriteReflectionParams {
                domain: "eng".into(),
                path: "d.md".into(),
                markdown: Some(PAGE.replace("Some run by day.", "A few run by day.")),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .automation_run_record(
            Loop::Reflect,
            "eng",
            &token,
            &RunReport {
                started_at: String::new(),
                outcome: RunOutcome::Done,
                done: 1,
                summary: "Answered one thread.".into(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        hook.next(minutes(40)).await.is_none(),
        "the run's own writes make nothing due"
    );
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn off_by_default_and_without_a_url() {
    // The URL is set, the MANIFEST says nothing: off.
    let mut hook = hook();
    let fx = reflecting("", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;
    assert!(hook.next(minutes(10)).await.is_none(), "off by default");
    let status = fx
        .engine
        .automation_status(Loop::Reflect, &["eng".to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["domains"][0]["due"], true, "{status}");
    assert_eq!(status["domains"][0]["delivery"], Value::Null, "{status}");
    task.stop().await;

    // The MANIFEST turns it on, no URL is set: nothing goes anywhere.
    let fx = reflecting("reflect_trigger: webhook\n", None).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;
    assert!(hook.next(minutes(10)).await.is_none(), "no URL, no webhook");
    let delivery = fx.engine.automation_delivery("eng", Loop::Reflect);
    assert!(!delivery.configured);
    assert_eq!(delivery.last_sent, None);
    assert_eq!(delivery.last_error, None);
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_delivery_retries_three_times_and_is_reported() {
    let mut hook = hook();
    hook.status.store(500, Ordering::SeqCst);
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;

    let first = hook.next(minutes(2)).await.expect("the first attempt");
    let mut at = first.at;
    for wait in [10, 30, 90] {
        let retry = hook.next(minutes(3)).await.expect("a retry");
        let gap = retry.at - at;
        assert!(
            gap >= Duration::from_secs(wait) && gap <= Duration::from_secs(wait + 2),
            "retry after {gap:?}, wanted {wait} s"
        );
        at = retry.at;
    }
    assert!(
        hook.next(minutes(4)).await.is_none(),
        "three retries, then a wait"
    );

    let delivery = fx.engine.automation_delivery("eng", Loop::Reflect);
    assert!(delivery.configured);
    assert_eq!(delivery.last_sent, None);
    assert_eq!(
        delivery.last_error.as_deref(),
        Some("the receiver answered 500")
    );
    let status = fx
        .engine
        .automation_status(Loop::Reflect, &["eng".to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["delivery"]["last_error"], "the receiver answered 500",
        "{status}"
    );
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_teammate_instance_sends_nothing() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    open_thread(&fx.engine, "Still true?").await;
    // The Reflection's home is another instance.
    std::fs::write(
        fx.root.join("MANIFEST.md"),
        manifest(
            "reflection: on\nreflection_home: https://home.example\nreflect_trigger: webhook\n",
        ),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let status = fx
        .engine
        .automation_status(Loop::Reflect, &["eng".to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["runs_on"], "https://home.example",
        "{status}"
    );

    let task = start(&fx.engine, &hook).await;
    // A person's question here is the home's to send, if this instance
    // takes it at all.
    let _ = fx
        .engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("d.md".into()),
                section: Some("bbbb2222".into()),
                text: Some("And this?".into()),
                ..Default::default()
            },
            &person(),
        )
        .await;
    assert!(
        hook.next(minutes(10)).await.is_none(),
        "the home sends, not a teammate"
    );
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_unusable_url_is_reported_and_never_retried() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some("http://exa mple/hook")).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;
    assert!(hook.next(minutes(10)).await.is_none(), "nothing is posted");
    let delivery = fx.engine.automation_delivery("eng", Loop::Reflect);
    assert!(
        delivery
            .last_error
            .as_deref()
            .is_some_and(|e| e.starts_with("the webhook URL is not usable")),
        "{delivery:?}"
    );
    assert_eq!(delivery.last_sent, None);
    task.stop().await;
}

/// `summary-doc` summarizes `full-text-doc`, which never says so: one
/// mechanical finding (V103).
fn write_summary_pair(root: &std::path::Path) {
    std::fs::write(
        root.join("summary-doc.md"),
        engram(
            "Summary doc",
            "summary-doc",
            "The distilled version of the migration write-up.\n\n- summarizes [[Full text doc]]\n- [context] written after the migration closed",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("full-text-doc.md"),
        engram(
            "Full text doc",
            "full-text-doc",
            "The full migration write-up with the shard tables.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary",
        ),
    )
    .unwrap();
}

/// `eng` with the evolve trigger on, one mechanical finding and the
/// threshold at `threshold`.
async fn maintaining(url: &str, threshold: u64) -> Fx {
    let fx = fixture_with(
        &[("MANIFEST.md", &manifest("evolve_trigger: webhook\n"))],
        |cfg, _| webhook(cfg, Some(url), Some(threshold)),
    )
    .await;
    write_summary_pair(&fx.root);
    fx.engine.sync(None).await.unwrap();
    fx
}

#[tokio::test(start_paused = true)]
async fn evolve_sends_when_the_mechanical_count_reaches_the_threshold() {
    let mut hook = hook();
    let fx = maintaining(&hook.url, 2).await;
    let task = start(&fx.engine, &hook).await;
    assert!(
        hook.next(minutes(11)).await.is_none(),
        "one finding is under a threshold of two"
    );
    task.stop().await;

    let fx = maintaining(&hook.url, 1).await;
    let begin = Instant::now();
    let task = start(&fx.engine, &hook).await;
    let hit = hook.next(minutes(6)).await.expect("one webhook");
    assert!(
        hit.at - begin <= Duration::from_secs(301),
        "checked on the five-minute tick: {:?}",
        hit.at - begin
    );
    assert_eq!(hit.header("x-crystalline-event"), "evolve");
    let body = hit.json();
    assert_eq!(body["event"], "evolve");
    assert_eq!(body["counts"]["mechanical"], 1, "{body}");
    assert!(
        hook.next(minutes(20)).await.is_none(),
        "once per crossing, not on every tick"
    );
    task.stop().await;
}

// --- the wire ---------------------------------------------------------------

/// One request a real listener took.
struct Taken {
    headers: HeaderMap,
    body: Bytes,
}

#[derive(Clone)]
struct ListenerState {
    tx: mpsc::UnboundedSender<Taken>,
    status: Arc<AtomicU16>,
}

async fn take_request(
    State(state): State<ListenerState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let _ = state.tx.send(Taken { headers, body });
    StatusCode::from_u16(state.status.load(Ordering::SeqCst)).unwrap()
}

/// A local listener at `/webhook/abc`: its address, what it took and the
/// status it answers.
async fn listener() -> (SocketAddr, mpsc::UnboundedReceiver<Taken>, Arc<AtomicU16>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let status = Arc::new(AtomicU16::new(200));
    let app = Router::new()
        .route("/webhook/abc", post(take_request))
        .with_state(ListenerState {
            tx,
            status: status.clone(),
        });
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(socket, app).await.unwrap();
    });
    (addr, rx, status)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wire_carries_the_signed_request() {
    let (addr, mut taken, status) = listener().await;
    let url = reqwest::Url::parse(&format!("http://{addr}/webhook/abc")).unwrap();
    let counts = serde_json::json!({"waiting_for_the_agent": 2, "feedback": 0, "updates": 1, "pages_to_update": 1});
    let now = crystalline_core::time::parse_rfc3339("2026-10-22T08:00:00Z")
        .unwrap()
        .stamp();
    let request = webhook_request(
        &url,
        SECRET,
        Loop::Reflect,
        "eng",
        "home.example",
        &counts,
        now,
    );
    HttpPoster::default().post(request.clone()).await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(5), taken.recv())
        .await
        .unwrap()
        .unwrap();
    let header = |name: &str| got.headers.get(name).unwrap().to_str().unwrap().to_string();
    assert_eq!(header("content-type"), "application/json");
    assert_eq!(header("x-crystalline-event"), "reflect");
    assert_eq!(header("x-crystalline-timestamp"), "1792656000");
    assert_eq!(
        header("x-crystalline-signature"),
        webhook_signature(SECRET.as_bytes(), "1792656000", &got.body)
    );
    assert_eq!(
        got.body.to_vec(),
        request.body,
        "the bytes signed are the bytes sent"
    );
    let body: Value = serde_json::from_slice(&got.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "event": "reflect",
            "domain": "eng",
            "instance": "home.example",
            "counts": counts,
            "sent_at": "2026-10-22T08:00:00Z",
        })
    );

    // A refusal is reported by its status.
    status.store(500, Ordering::SeqCst);
    let refused = HttpPoster::default().post(request).await.unwrap_err();
    assert_eq!(refused, "the receiver answered 500");

    // A receiver that is not there is reported without the URL, whose path
    // often works as a bearer.
    let gone = reqwest::Url::parse("http://127.0.0.1:1/webhook/s3cr3t-path").unwrap();
    let failed = HttpPoster::default()
        .post(webhook_request(
            &gone,
            SECRET,
            Loop::Evolve,
            "eng",
            "x",
            &counts,
            now,
        ))
        .await
        .unwrap_err();
    assert!(failed.starts_with("could not connect"), "{failed}");
    assert!(
        !failed.contains("s3cr3t-path") && !failed.contains(SECRET),
        "{failed}"
    );
}

#[tokio::test(start_paused = true)]
async fn evolve_sends_again_after_the_count_drops_and_crosses_again() {
    let mut hook = hook();
    let fx = maintaining(&hook.url, 1).await;
    let task = start(&fx.engine, &hook).await;
    hook.next(minutes(6)).await.expect("the first crossing");

    // A run settles it: the next tick sees nothing due.
    std::fs::remove_file(fx.root.join("summary-doc.md")).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        hook.next(minutes(6)).await.is_none(),
        "nothing due, nothing sent"
    );

    // New work crosses the threshold again.
    write_summary_pair(&fx.root);
    fx.engine.sync(None).await.unwrap();
    let hit = hook.next(minutes(6)).await.expect("the second crossing");
    assert_eq!(hit.json()["counts"]["mechanical"], 1);
    task.stop().await;
}

// --- fix round 1 ------------------------------------------------------------

/// A delivery that fails all four attempts, the first inside `first`: the
/// hits.
async fn four_failed_attempts(hook: &mut Hook, first: Duration) -> Vec<Hit> {
    let mut hits = vec![hook.next(first).await.expect("a first attempt")];
    for _ in 0..3 {
        hits.push(hook.next(minutes(3)).await.expect("a retry"));
    }
    hits
}

#[tokio::test(start_paused = true)]
async fn a_held_lease_stops_the_retries() {
    let mut hook = hook();
    hook.status.store(500, Ordering::SeqCst);
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;
    hook.next(minutes(2)).await.expect("the first attempt");
    // A runner starts before the first retry.
    let token = take(&fx.engine).await;
    assert!(
        hook.next(minutes(5)).await.is_none(),
        "no retry while the lease is held"
    );
    // When it goes with the thread still waiting, the release sends.
    hook.status.store(204, Ordering::SeqCst);
    fx.engine
        .automation_lease_release(Loop::Reflect, "eng", &token, &Scope::Unrestricted)
        .await
        .unwrap();
    hook.next(minutes(2)).await.expect("one on release");
    task.stop().await;
}

/// A run that failed releases its lease while the loop backs off: nothing
/// is sent during the wait, and the waiting thread is sent once it ends,
/// with no new event to wake the trigger.
#[tokio::test(start_paused = true)]
async fn a_failed_run_waits_out_its_backoff_then_sends() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    let token = take(&fx.engine).await;
    open_thread(&fx.engine, "Still true?").await;
    fx.engine
        .automation_run_record(
            Loop::Reflect,
            "eng",
            &token,
            &RunReport {
                started_at: String::new(),
                outcome: RunOutcome::Failed,
                done: 0,
                summary: String::new(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        hook.next(minutes(6)).await.is_none(),
        "nothing while the loop backs off"
    );
    // The five minutes after the failure are over.
    fx.engine.set_automation_now(Some(
        crystalline_core::time::parse_rfc3339("2026-10-21T10:06:00+00:00").unwrap(),
    ));
    let hit = hook
        .next(minutes(6))
        .await
        .expect("sent once the wait ends");
    assert_eq!(hit.json()["counts"]["waiting_for_the_agent"], 1);
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_release_inside_five_minutes_waits_for_the_cap() {
    let mut hook = hook();
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;
    let first = hook.next(minutes(2)).await.expect("the first webhook");
    // A run starts, a second question arrives, the run stops without
    // answering: the release is owed a send, but the cap holds it back.
    let token = take(&fx.engine).await;
    open_thread(&fx.engine, "And this?").await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    fx.engine
        .automation_lease_release(Loop::Reflect, "eng", &token, &Scope::Unrestricted)
        .await
        .unwrap();
    let held = hook.next(minutes(10)).await.expect("the held-back send");
    let gap = held.at - first.at;
    assert!(
        gap >= RATE_LIMIT && gap <= RATE_LIMIT + Duration::from_secs(2),
        "sent {gap:?} after the first"
    );
    assert_eq!(held.json()["counts"]["waiting_for_the_agent"], 2);
    assert!(hook.next(minutes(10)).await.is_none(), "one, not more");
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_delivery_that_failed_every_retry_is_owed_with_a_growing_wait() {
    let mut hook = hook();
    hook.status.store(500, Ordering::SeqCst);
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    open_thread(&fx.engine, "Still true?").await;
    let mut last = four_failed_attempts(&mut hook, minutes(2))
        .await
        .pop()
        .unwrap()
        .at;
    for wait in [5, 10] {
        let again = four_failed_attempts(&mut hook, minutes(wait + 1)).await;
        let gap = again[0].at - last;
        assert!(
            gap >= minutes(wait) && gap <= minutes(wait) + Duration::from_secs(2),
            "sent again {gap:?} after a failed delivery, wanted {wait} minutes"
        );
        last = again[3].at;
    }
    // The receiver is back; the third wait is 20 minutes.
    hook.status.store(204, Ordering::SeqCst);
    let sent = hook.next(minutes(25)).await.expect("sent again");
    let gap = sent.at - last;
    assert!(
        gap >= minutes(20) && gap <= minutes(20) + Duration::from_secs(2),
        "{gap:?}"
    );
    let delivery = recorded(&fx.engine, Loop::Reflect).await;
    assert!(delivery.last_sent.is_some(), "{delivery:?}");
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn evolve_is_sent_again_after_a_failed_delivery() {
    let mut hook = hook();
    hook.status.store(500, Ordering::SeqCst);
    let fx = maintaining(&hook.url, 1).await;
    let task = start(&fx.engine, &hook).await;
    let last = hook.next(minutes(6)).await.expect("the first attempt");
    let mut last = last.at;
    for _ in 0..3 {
        last = hook.next(minutes(3)).await.expect("a retry").at;
    }
    hook.status.store(204, Ordering::SeqCst);
    let again = hook
        .next(minutes(10))
        .await
        .expect("the crossing is still owed");
    let gap = again.at - last;
    assert!(
        gap >= minutes(5) && gap <= minutes(5) + Duration::from_secs(2),
        "{gap:?}"
    );
    assert_eq!(again.json()["counts"]["mechanical"], 1);
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_delivery_that_panics_frees_its_slot() {
    let mut hook = hook();
    hook.panic_next.store(true, Ordering::SeqCst);
    let fx = reflecting("reflect_trigger: webhook\n", Some(&hook.url)).await;
    let task = start(&fx.engine, &hook).await;
    let opened = Instant::now();
    open_thread(&fx.engine, "Still true?").await;
    let hit = hook
        .next(minutes(10))
        .await
        .expect("a send after the broken one");
    let after = hit.at - opened;
    assert!(
        after >= minutes(5) && after <= minutes(7),
        "sent {after:?} after the thread"
    );
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_evolve_runs_own_fixes_send_nothing() {
    let mut hook = hook();
    let fx = maintaining(&hook.url, 1).await;
    let task = start(&fx.engine, &hook).await;
    hook.next(minutes(6)).await.expect("the crossing");
    let token = fx
        .engine
        .automation_lease_take(Loop::Evolve, "eng", "n8n", &Scope::Unrestricted)
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    // The run wires the missing reciprocal relation.
    std::fs::write(
        fx.root.join("full-text-doc.md"),
        engram(
            "Full text doc",
            "full-text-doc",
            "The full migration write-up with the shard tables.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary\n- summarized_by [[Summary doc]]",
        ),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let status = fx
        .engine
        .automation_status(Loop::Evolve, &["eng".to_string()], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(status["domains"][0]["counts"]["mechanical"], 0, "{status}");
    fx.engine
        .automation_run_record(
            Loop::Evolve,
            "eng",
            &token,
            &RunReport {
                started_at: String::new(),
                outcome: RunOutcome::Done,
                done: 1,
                summary: "Wired one relation.".into(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        hook.next(minutes(30)).await.is_none(),
        "the run's own fixes make nothing due"
    );
    task.stop().await;
}

/// A listener at `/webhook/abc` that answers 302 to `/elsewhere`, and
/// counts what reaches `/elsewhere`.
async fn redirecting() -> (SocketAddr, Arc<AtomicU16>) {
    let reached = Arc::new(AtomicU16::new(0));
    let counter = reached.clone();
    let app = Router::new()
        .route(
            "/webhook/abc",
            post(|| async { (StatusCode::FOUND, [("location", "/elsewhere")]) }),
        )
        .route(
            "/elsewhere",
            post(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            })
            .get(|| async { StatusCode::OK }),
        );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(socket, app).await.unwrap();
    });
    (addr, reached)
}

fn some_request(url: &str) -> WebhookRequest {
    let url = reqwest::Url::parse(url).unwrap();
    webhook_request(
        &url,
        SECRET,
        Loop::Reflect,
        "eng",
        "home.example",
        &serde_json::json!({"waiting_for_the_agent": 1}),
        crystalline_core::time::now(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_is_not_followed_and_counts_as_failed() {
    let (addr, reached) = redirecting().await;
    let failed = HttpPoster::default()
        .post(some_request(&format!("http://{addr}/webhook/abc")))
        .await
        .unwrap_err();
    assert_eq!(failed, "the receiver answered 302");
    assert_eq!(reached.load(Ordering::SeqCst), 0, "not followed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receiver_that_never_answers_times_out() {
    assert_eq!(HttpPoster::default().timeout(), Duration::from_secs(10));
    // Accepts the connection and never answers.
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = socket.accept().await {
            held.push(stream);
        }
    });
    let began = std::time::Instant::now();
    let failed = HttpPoster::with_timeout(Duration::from_millis(300))
        .post(some_request(&format!("http://{addr}/webhook/abc")))
        .await
        .unwrap_err();
    assert!(began.elapsed() < Duration::from_secs(5));
    assert!(failed.starts_with("timed out"), "{failed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_connection_names_its_kind() {
    let failed = HttpPoster::default()
        .post(some_request("http://127.0.0.1:1/webhook/s3cr3t-path"))
        .await
        .unwrap_err();
    assert!(failed.starts_with("could not connect"), "{failed}");
    assert!(failed.to_lowercase().contains("refused"), "{failed}");
    assert!(!failed.contains("s3cr3t-path"), "{failed}");
}

#[test]
fn debug_shows_only_the_scheme_and_host_of_the_url() {
    let request = some_request("https://user:pw@hooks.example/webhook/s3cr3t?token=t0k3n");
    let shown = format!("{request:?}");
    assert!(shown.contains("https://hooks.example"), "{shown}");
    for hidden in ["user", "pw@", "s3cr3t", "t0k3n", SECRET] {
        assert!(!shown.contains(hidden), "{hidden} in {shown}");
    }
}
