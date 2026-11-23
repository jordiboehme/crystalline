//! The trigger webhooks: when loop work waits on a domain whose MANIFEST
//! says `reflect_trigger: webhook` or `evolve_trigger: webhook`, the daemon
//! posts a signed webhook to the instance's `automation.webhook.url`, so an
//! automation service can start a runner without polling.
//!
//! The task never acts on an event alone (ruling R1): an event only arms a
//! look, and the look reads [`Engine::automation_status`], which is computed
//! from state. So a run's own writes cannot make its loop due, nothing is
//! sent while a lease is held (status reports `running` then) and nothing is
//! sent for a domain whose loop runs on another instance (`runs_on`).
//!
//! When it sends:
//! - Reflect, people waiting (a thread for the agent, feedback): one look
//!   [`PEOPLE_WINDOW`] after the first event, which coalesces the burst.
//! - Reflect, page updates only: after [`SOURCE_QUIET`] without a source
//!   change on the domain (an engram event there, or a move of its update
//!   count).
//! - Reflect, a page whose freshness ran out (`stale`, counted on its own):
//!   at the first look that sees it, without the quiet rule. Every look arms
//!   the next at the time the status names for the next page that falls
//!   due (`next_stale_in_secs`), so a page falls due with no event at all.
//! - Evolve: on the [`EVOLVE_TICK`], when the mechanical count reaches
//!   `automation.evolve_threshold` (once per crossing, not on every tick it
//!   stays above it).
//! - When a lease goes away and work is still due, at once (seen on the
//!   [`LEASE_TICK`]); for evolve the threshold still applies.
//! - While the loop backs off after failed runs, nothing; the slot looks
//!   again every [`RATE_LIMIT`] and sends what it was owed once the wait
//!   ends.
//!
//! Reflect sends again whenever a look finds people still waiting (any
//! event arms one), held only by the cap below; evolve sends again only on
//! a new crossing, a release or a failed delivery.
//!
//! At most one delivery per domain and loop per [`RATE_LIMIT`], and that cap
//! wins over everything above, a release included. A failed attempt is
//! retried after each of [`RETRY_DELAYS`], unless a run took the lease in
//! the meantime; every attempt is logged, and the outcome is recorded with
//! [`Engine::record_delivery`]. A delivery whose attempts all failed stays
//! owed and is sent again, while the work is due, after the next of
//! [`FAILED_DELIVERY_WAITS`]. The body carries counts only, never content
//! or paths, and the secret is never logged.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crystalline_core::Trigger;
use crystalline_core::automation::{BACKOFF_REASON_PREFIX, Loop, webhook_signature};
use crystalline_core::time::{self, Stamp};
use crystalline_engine::changes::Change;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, MissedTickBehavior};

use crate::Engine;
use crate::scope::Scope;

/// How long after the first event a look runs, so what arrives in that
/// window is sent as one, and the send still leaves within 60 s.
pub const PEOPLE_WINDOW: Duration = Duration::from_secs(55);

/// How long a domain's sources must be quiet before page updates alone are
/// sent.
pub const SOURCE_QUIET: Duration = Duration::from_secs(30 * 60);

/// How often the evolve threshold is checked.
pub const EVOLVE_TICK: Duration = Duration::from_secs(300);

/// How often the leases are looked at, so a released lease with work still
/// due is sent at once.
pub const LEASE_TICK: Duration = Duration::from_secs(60);

/// The least time between two deliveries for one domain and loop.
pub const RATE_LIMIT: Duration = Duration::from_secs(300);

/// The waits before the retries of a failed delivery.
pub const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(90),
];

/// The waits before a delivery that failed every attempt is sent again:
/// one per failed delivery in a row, the last one repeating. A delivery
/// that gets through starts the list over.
pub const FAILED_DELIVERY_WAITS: [Duration; 5] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(20 * 60),
    Duration::from_secs(40 * 60),
    Duration::from_secs(60 * 60),
];

/// How long one attempt may take before it counts as failed.
pub const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

/// The header that names the loop.
pub const EVENT_HEADER: &str = "X-Crystalline-Event";
/// The header with the unix seconds the signature covers.
pub const TIMESTAMP_HEADER: &str = "X-Crystalline-Timestamp";
/// The header with `sha256=<hex>`.
pub const SIGNATURE_HEADER: &str = "X-Crystalline-Signature";

/// The error recorded while a URL is set but no secret is.
const NO_SECRET: &str = crystalline_engine::engine::WEBHOOK_NO_SECRET;

/// Send the trigger webhooks until `shutdown` turns true. Spawned by the
/// daemon; it does no work before its first window, so the start never
/// waits on it, and the deliveries in flight are dropped on shutdown.
pub async fn run_automation_triggers(engine: Arc<Engine>, shutdown: watch::Receiver<bool>) {
    run_automation_triggers_with(engine, shutdown, Arc::new(HttpPoster::default())).await;
}

/// [`run_automation_triggers`] posting through `poster`: the daemon's posts
/// over HTTP, a test's records what would go out.
pub async fn run_automation_triggers_with(
    engine: Arc<Engine>,
    mut shutdown: watch::Receiver<bool>,
    poster: Arc<dyn Poster>,
) {
    let mut events = engine.changes().subscribe();
    let start = Instant::now();
    let mut lease_tick = tokio::time::interval_at(start + LEASE_TICK, LEASE_TICK);
    lease_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut evolve_tick = tokio::time::interval_at(start + EVOLVE_TICK, EVOLVE_TICK);
    evolve_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut triggers = Triggers::new(engine, poster);
    let mut deliveries: JoinSet<Outcome> = JoinSet::new();
    let mut listening = true;
    loop {
        let wake = triggers.next_wake();
        tokio::select! {
            _ = wait_true(&mut shutdown) => break,
            event = events.recv(), if listening => match event {
                Ok(envelope) => triggers.on_change(&envelope.change, Instant::now()),
                Err(RecvError::Lagged(_)) => triggers.on_lag(Instant::now()),
                Err(RecvError::Closed) => listening = false,
            },
            _ = lease_tick.tick() => triggers.lease_tick(&mut deliveries).await,
            _ = evolve_tick.tick() => triggers.evolve_tick(&mut deliveries).await,
            _ = sleep_until(wake), if wake.is_some() => triggers.due_looks(&mut deliveries).await,
            Some(done) = deliveries.join_next_with_id() => {
                let (id, outcome) = match done {
                    Ok((id, outcome)) => (id, outcome),
                    // A delivery that panicked or was cut off counts as one
                    // that failed, so its slot is never left in flight.
                    Err(e) => (e.id(), Outcome::Broken),
                };
                triggers.delivered(id, outcome, Instant::now());
            }
        }
    }
    deliveries.abort_all();
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn wait_true(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// One domain and loop.
#[derive(Default)]
struct Slot {
    /// When the next look runs.
    look_at: Option<Instant>,
    /// A look found a lease held: when it goes, work still due is sent at
    /// once.
    after_lease: bool,
    /// A send is owed (a release, or a send the rate limit held back): the
    /// next look sends while work is due, past the quiet rule and the
    /// crossing.
    urgent: bool,
    /// When the last delivery started.
    last_attempt: Option<Instant>,
    /// Deliveries in a row that failed every attempt.
    failures: usize,
    /// No delivery before this, after a delivery that failed.
    not_before: Option<Instant>,
    /// A delivery is running (its retries included).
    in_flight: bool,
    /// The count the last look saw: reflect's updates, evolve's
    /// mechanical findings.
    seen: Option<u64>,
    /// Reflect: the last source change.
    quiet_since: Option<Instant>,
}

impl Slot {
    fn arm(&mut self, at: Instant) {
        self.look_at = Some(self.look_at.map_or(at, |was| was.min(at)));
    }
}

/// How a delivery ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The receiver took it.
    Sent,
    /// Every attempt failed (or the delivery broke off).
    Failed,
    /// A runner took the lease before a retry: the release sends.
    LeaseHeld,
    /// The delivery task panicked or was cut off; counts as failed.
    Broken,
}

struct Triggers {
    engine: Arc<Engine>,
    poster: Arc<dyn Poster>,
    slots: HashMap<(String, Loop), Slot>,
    /// The domain and loop of each delivery in flight, by task id.
    in_flight: HashMap<tokio::task::Id, (String, Loop)>,
    /// The domains with a webhook trigger, as the last lease tick read them.
    reflect_domains: Vec<String>,
    evolve_domains: Vec<String>,
    /// The URL a warning was logged for, so a bad one is named once.
    warned: Option<String>,
}

impl Triggers {
    fn new(engine: Arc<Engine>, poster: Arc<dyn Poster>) -> Triggers {
        Triggers {
            engine,
            poster,
            slots: HashMap::new(),
            in_flight: HashMap::new(),
            reflect_domains: Vec::new(),
            evolve_domains: Vec::new(),
            warned: None,
        }
    }

    fn slot(&mut self, domain: &str, lp: Loop) -> &mut Slot {
        self.slots.entry((domain.to_string(), lp)).or_default()
    }

    fn next_wake(&self) -> Option<Instant> {
        self.slots.values().filter_map(|s| s.look_at).min()
    }

    /// An event arms a reflect look one window on. A change to an engram
    /// can move the updates of a page on any domain (sources cross domains),
    /// so it arms every reflect domain and restarts its own domain's quiet
    /// clock.
    fn on_change(&mut self, change: &Change, now: Instant) {
        let domain = change.domain().to_string();
        let at = now + PEOPLE_WINDOW;
        match change {
            Change::Reflection(_) => self.slot(&domain, Loop::Reflect).arm(at),
            Change::Engram(_) | Change::Domain(_) => {
                let slot = self.slot(&domain, Loop::Reflect);
                slot.quiet_since = Some(now);
                slot.arm(at);
                for other in self.reflect_domains.clone() {
                    self.slot(&other, Loop::Reflect).arm(at);
                }
            }
        }
    }

    /// Events were missed: treat it as a source change everywhere.
    fn on_lag(&mut self, now: Instant) {
        let at = now + PEOPLE_WINDOW;
        for domain in self.reflect_domains.clone() {
            let slot = self.slot(&domain, Loop::Reflect);
            slot.quiet_since = Some(now);
            slot.arm(at);
        }
    }

    /// Read the trigger domains again, and send at once where a lease went
    /// away with work still due. A reflect domain seen for the first time
    /// gets its first look here.
    async fn lease_tick(&mut self, deliveries: &mut JoinSet<Outcome>) {
        if self.engine.webhook_target().is_none() {
            return;
        }
        self.reflect_domains = self.engine.webhook_domains(Loop::Reflect).await;
        self.evolve_domains = self.engine.webhook_domains(Loop::Evolve).await;
        let now = Instant::now();
        let pairs: Vec<(String, Loop)> = self
            .reflect_domains
            .iter()
            .map(|d| (d.clone(), Loop::Reflect))
            .chain(
                self.evolve_domains
                    .iter()
                    .map(|d| (d.clone(), Loop::Evolve)),
            )
            .collect();
        for (domain, lp) in pairs {
            let first = !self.slots.contains_key(&(domain.clone(), lp));
            let fresh = match self.engine.automation_lease_fresh(&domain, lp).await {
                Ok(fresh) => fresh,
                Err(e) => {
                    tracing::debug!("{lp} trigger: lease of '{domain}' not read: {e}");
                    continue;
                }
            };
            let slot = self.slot(&domain, lp);
            if fresh {
                slot.after_lease = true;
            } else if slot.after_lease {
                slot.after_lease = false;
                self.look(&domain, lp, true, deliveries).await;
            } else if first && lp == Loop::Reflect {
                slot.arm(now);
            }
        }
    }

    async fn evolve_tick(&mut self, deliveries: &mut JoinSet<Outcome>) {
        let _pass = crystalline_engine::timing::Pass::start("evolve tick");
        for domain in self.evolve_domains.clone() {
            self.look(&domain, Loop::Evolve, false, deliveries).await;
        }
    }

    async fn due_looks(&mut self, deliveries: &mut JoinSet<Outcome>) {
        let now = Instant::now();
        let due: Vec<(String, Loop)> = self
            .slots
            .iter_mut()
            .filter(|(_, slot)| slot.look_at.is_some_and(|at| at <= now))
            .map(|(key, slot)| {
                slot.look_at = None;
                key.clone()
            })
            .collect();
        for (domain, lp) in due {
            self.look(&domain, lp, false, deliveries).await;
        }
    }

    fn delivered(&mut self, id: tokio::task::Id, outcome: Outcome, now: Instant) {
        let Some((domain, lp)) = self.in_flight.remove(&id) else {
            return;
        };
        if outcome == Outcome::Broken {
            self.engine.record_delivery(
                &domain,
                lp,
                Err("the delivery stopped before it was sent".to_string()),
            );
        }
        let slot = self.slot(&domain, lp);
        slot.in_flight = false;
        match outcome {
            Outcome::Sent => {
                slot.failures = 0;
                slot.not_before = None;
            }
            Outcome::Failed | Outcome::Broken => {
                // Still owed: sent again after a wait that grows with each
                // delivery in a row that failed, if the work is still due.
                let wait =
                    FAILED_DELIVERY_WAITS[slot.failures.min(FAILED_DELIVERY_WAITS.len() - 1)];
                slot.failures += 1;
                slot.not_before = Some(now + wait);
                slot.urgent = true;
                slot.arm(now + wait);
            }
            Outcome::LeaseHeld => slot.after_lease = true,
        }
    }

    /// Look at one domain and loop and send when the rules say so.
    async fn look(
        &mut self,
        domain: &str,
        lp: Loop,
        urgent: bool,
        deliveries: &mut JoinSet<Outcome>,
    ) {
        let Some(target) = self.engine.webhook_target() else {
            return;
        };
        if self.engine.loop_trigger(domain, lp).await != Trigger::Webhook {
            self.slots.remove(&(domain.to_string(), lp));
            return;
        }
        let Some(secret) = target.secret.clone() else {
            if self.warned.as_deref() != Some(NO_SECRET) {
                tracing::warn!("automation trigger: {NO_SECRET}");
                self.warned = Some(NO_SECRET.to_string());
            }
            self.engine
                .record_delivery(domain, lp, Err(NO_SECRET.to_string()));
            return;
        };
        let url = match usable_url(&target.url) {
            Ok(url) => url,
            Err(why) => {
                if self.warned.as_deref() != Some(target.url.as_str()) {
                    tracing::warn!("automation trigger: {why}; nothing is sent");
                    self.warned = Some(target.url.clone());
                }
                self.engine.record_delivery(domain, lp, Err(why));
                return;
            }
        };
        let status = match self
            .engine
            .automation_status(lp, &[domain.to_string()], &Scope::Unrestricted)
            .await
        {
            Ok(status) => status,
            Err(e) => {
                tracing::debug!("{lp} trigger: status of '{domain}' not read: {e}");
                return;
            }
        };
        let entry = &status["domains"][0];
        // The next page that falls due arms a look of its own, whatever this
        // look decides: nothing else may come to wake the slot then. The
        // status names a time ahead of its clock, never now.
        if let Some(secs) = entry["next_stale_in_secs"].as_u64()
            && let Some(at) = Instant::now().checked_add(Duration::from_secs(secs.max(1)))
        {
            self.slot(domain, lp).arm(at);
        }
        // The loop backs off after failed runs: nothing is sent while it
        // waits, and nothing new may come to wake the slot when it ends, so
        // the slot looks again on its own, keeping a send it was owed.
        if entry["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with(BACKOFF_REASON_PREFIX))
        {
            let now = Instant::now();
            let slot = self.slot(domain, lp);
            slot.urgent |= urgent;
            slot.arm(now + RATE_LIMIT);
            return;
        }
        if !entry["runs_on"].is_null() || !entry["reason"].is_null() {
            return;
        }
        let now = Instant::now();
        let threshold = self.engine.evolve_threshold();
        let slot = self.slot(domain, lp);
        let urgent = urgent || slot.urgent;
        if !entry["running"].is_null() {
            slot.after_lease = true;
            return;
        }
        let counts = entry["counts"].clone();
        let count = |name: &str| counts[name].as_u64().unwrap_or(0);
        // The count is recorded on every look, a look with nothing due
        // included (its counts are zero), so the next crossing and the next
        // quiet clock start from what is there now.
        let was = slot.seen;
        let now_seen = match lp {
            Loop::Reflect => count("updates"),
            Loop::Evolve => count("mechanical"),
        };
        slot.seen = Some(now_seen);
        if lp == Loop::Reflect && was != Some(now_seen) {
            slot.quiet_since = Some(now);
        }
        if entry["due"] != true {
            slot.urgent = false;
            return;
        }
        let send = match lp {
            Loop::Reflect => {
                let people = count("waiting_for_the_agent") + count("feedback");
                // A page whose freshness ran out does not wait for quiet
                // sources: its time is its own.
                let stale = count("stale");
                let quiet_until = slot.quiet_since.unwrap_or(now) + SOURCE_QUIET;
                if people > 0 || stale > 0 || urgent || now >= quiet_until {
                    true
                } else {
                    slot.arm(quiet_until);
                    false
                }
            }
            Loop::Evolve => {
                // Sent when the count crosses the threshold, not on every
                // tick it stays above it; a release sends at once.
                let crossed = was.is_none_or(|was| was < threshold);
                now_seen >= threshold && (crossed || urgent)
            }
        };
        if !send {
            return;
        }
        // A delivery in flight, the cap since the last one, or the wait
        // after a failed one holds this send back.
        let hold_until = slot
            .last_attempt
            .map(|last| last + RATE_LIMIT)
            .into_iter()
            .chain(slot.not_before)
            .max();
        if slot.in_flight || hold_until.is_some_and(|until| now < until) {
            // The send is owed: the look at the hold's end sends it
            // whatever the quiet rule or the crossing says by then.
            slot.urgent = true;
            slot.arm(hold_until.unwrap_or(now).max(now + RETRY_DELAYS[0]));
            return;
        }
        slot.urgent = false;
        slot.last_attempt = Some(now);
        slot.in_flight = true;
        let send = Delivering {
            engine: self.engine.clone(),
            poster: self.poster.clone(),
            url,
            secret,
            domain: domain.to_string(),
            lp,
            counts,
        };
        let task = deliveries.spawn(send.run());
        self.in_flight.insert(task.id(), (domain.to_string(), lp));
    }
}

/// The URL parsed, when it is one a webhook can go to: http or https with a
/// host. The setting checks only the prefix.
fn usable_url(text: &str) -> Result<reqwest::Url, String> {
    let url =
        reqwest::Url::parse(text).map_err(|e| format!("the webhook URL is not usable ({e})"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(
            "the webhook URL is not usable (it needs http:// or https:// and a host)".to_string(),
        );
    }
    Ok(url)
}

/// One webhook, ready to post: the bytes signed are the bytes sent.
#[derive(Clone)]
pub struct WebhookRequest {
    /// Where it goes.
    pub url: reqwest::Url,
    /// `Content-Type` and the three `X-Crystalline-*` headers.
    pub headers: Vec<(&'static str, String)>,
    /// `{event, domain, instance, counts, sent_at}`, serialized once.
    pub body: Vec<u8>,
}

/// Hand-written: the URL shows only its scheme and host (a path or a query
/// often works as a bearer, and userinfo is a password), the headers only
/// their names and the body only its length.
impl std::fmt::Debug for WebhookRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| *name).collect();
        f.debug_struct("WebhookRequest")
            .field("url", &url_origin(&self.url))
            .field("headers", &names)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// `scheme://host[:port]` of `url`, nothing more.
pub fn url_origin(url: &reqwest::Url) -> String {
    let host = url.host_str().unwrap_or("?");
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

/// The webhook for `lp` on `domain`, signed with `secret` at `now`: the
/// timestamp header is `now` in unix seconds, `sent_at` the same instant in
/// RFC 3339, and the signature covers `<timestamp>.<body>`.
pub fn webhook_request(
    url: &reqwest::Url,
    secret: &str,
    lp: Loop,
    domain: &str,
    instance: &str,
    counts: &Value,
    now: Stamp,
) -> WebhookRequest {
    let timestamp = now.unix_seconds().to_string();
    let body = serde_json::to_vec(&json!({
        "event": lp.as_str(),
        "domain": domain,
        "instance": instance,
        "counts": counts,
        "sent_at": now.rfc3339_secs_z(),
    }))
    .expect("a JSON value serializes");
    let signature = webhook_signature(secret.as_bytes(), &timestamp, &body);
    WebhookRequest {
        url: url.clone(),
        headers: vec![
            ("Content-Type", "application/json".to_string()),
            (EVENT_HEADER, lp.as_str().to_string()),
            (TIMESTAMP_HEADER, timestamp),
            (SIGNATURE_HEADER, signature),
        ],
        body,
    }
}

/// What a webhook goes out through.
#[async_trait::async_trait]
pub trait Poster: Send + Sync {
    /// Post `request` once: `Ok` when the receiver took it (a 2xx answer),
    /// else why not, in words that never carry the URL or the secret.
    async fn post(&self, request: WebhookRequest) -> Result<(), String>;
}

/// The daemon's [`Poster`]: a POST through the client crystalline-remote
/// builds (no redirects followed, so a 3xx counts as failed), built once on
/// the first post and reused, at most [`HttpPoster::timeout`] per attempt.
pub struct HttpPoster {
    client: tokio::sync::OnceCell<reqwest::Client>,
    timeout: Duration,
}

impl Default for HttpPoster {
    /// [`ATTEMPT_TIMEOUT`] per attempt.
    fn default() -> HttpPoster {
        HttpPoster::with_timeout(ATTEMPT_TIMEOUT)
    }
}

impl HttpPoster {
    /// A poster that gives each attempt `timeout`.
    pub fn with_timeout(timeout: Duration) -> HttpPoster {
        HttpPoster {
            client: tokio::sync::OnceCell::new(),
            timeout,
        }
    }

    /// How long one attempt may take.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    async fn client(&self) -> Result<&reqwest::Client, String> {
        self.client
            .get_or_try_init(|| async {
                // The first build reads the system proxy settings, which is
                // slow on macOS; it runs on a blocking thread once.
                crystalline_remote::warm_http_client().await;
                crystalline_remote::http_client().map_err(|e| e.to_string())
            })
            .await
    }
}

#[async_trait::async_trait]
impl Poster for HttpPoster {
    async fn post(&self, request: WebhookRequest) -> Result<(), String> {
        let client = self.client().await?;
        let mut builder = client.post(request.url).timeout(self.timeout);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        let answer = builder.body(request.body).send().await.map_err(describe)?;
        let code = answer.status();
        if code.is_success() {
            Ok(())
        } else {
            Err(format!("the receiver answered {}", code.as_u16()))
        }
    }
}

/// A failed request in words an operator can act on: its kind (timed out,
/// could not connect, which covers a refused connection, a name that does
/// not resolve and a TLS failure) and the innermost cause, which names a
/// host at most, never the URL's path or query.
fn describe(error: reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "the request failed"
    };
    let mut innermost = None;
    let mut cause = std::error::Error::source(&error);
    while let Some(next) = cause {
        innermost = Some(next.to_string());
        cause = next.source();
    }
    let cause = innermost.unwrap_or_else(|| error.without_url().to_string());
    format!("{kind}: {cause}")
}

/// One delivery: the first attempt and its retries.
struct Delivering {
    engine: Arc<Engine>,
    poster: Arc<dyn Poster>,
    url: reqwest::Url,
    secret: String,
    domain: String,
    lp: Loop,
    counts: Value,
}

impl Delivering {
    async fn run(self) -> Outcome {
        let lp = self.lp;
        let domain = self.domain.clone();
        // The host only: a webhook path often works as a bearer.
        let host = self.url.host_str().unwrap_or("?").to_string();
        let attempts = 1 + RETRY_DELAYS.len();
        for attempt in 1..=attempts {
            if attempt > 1 {
                tokio::time::sleep(RETRY_DELAYS[attempt - 2]).await;
                // Never while a run holds the lease: the release sends if
                // the work is still due.
                if matches!(
                    self.engine.automation_lease_fresh(&domain, lp).await,
                    Ok(true)
                ) {
                    tracing::info!(
                        "{lp} trigger for '{domain}': a run took the lease, so the retries stop"
                    );
                    return Outcome::LeaseHeld;
                }
            }
            let now = time::now();
            let request = webhook_request(
                &self.url,
                &self.secret,
                lp,
                &domain,
                &self.engine.instance_name(),
                &self.counts,
                now,
            );
            match self.poster.post(request).await {
                Ok(()) => {
                    tracing::info!(
                        "{lp} trigger for '{domain}' sent to {host} (attempt {attempt} of {attempts})"
                    );
                    let sent_at = now.rfc3339_secs_z();
                    self.engine.record_delivery(&domain, lp, Ok(sent_at));
                    return Outcome::Sent;
                }
                Err(why) => {
                    tracing::warn!(
                        "{lp} trigger for '{domain}' to {host} failed (attempt {attempt} of {attempts}): {why}"
                    );
                    self.engine.record_delivery(&domain, lp, Err(why));
                }
            }
        }
        Outcome::Failed
    }
}
