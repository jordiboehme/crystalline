//! `GET /api/v1/events`: the server-sent event stream a tab keeps open so the page
//! it is reading follows the knowledge.
//!
//! One subscriber per open response. Each is filtered by what its session
//! may read - the same `hidden_domains` fold the domain listing goes through,
//! re-resolved lazily so a revocation lands within ten seconds - except an
//! event that carries the audience the engine itself resolved eagerly, at the
//! moment of the change (a domain's rename, under its old name, and its
//! removal). That audience is checked directly against this session's
//! identity: neither this subscriber's cache nor a fresh call is consulted,
//! since both can answer from a registry the change is in the middle of
//! altering (Jordi's ruling, 2026-09-27). Such an event also marks the
//! cache stale, so the frame that follows it (a rename's new name) is checked
//! against the records as they are after the change rather than a cache from
//! before it. A draft event reaches the draft's owner alone. A client that
//! presents the last id it saw is caught up from the engine's ring or told to
//! `reset`; one that falls behind the channel is told to `reset` too. Every
//! fifteen seconds a comment keeps the connection alive through proxies, and
//! the daemon's shutdown ends the stream so the drain never waits on a
//! browser.
//!
//! Who is listening is re-read too, not only what they may see: at every
//! re-resolution the request's own credentials are resolved again through the
//! accounts store, so a logout, a revoked session or a disabled or deleted
//! account ends the stream, and a changed role takes effect, within the same
//! ten seconds. A re-resolution that fails drops the event it was made for
//! and is tried again on the next one; it never delivers on an old answer.
//! The number of open streams is capped per account and in total.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use super::auth::Identity;
use super::{ApiError, ProblemDetail, RestState};
use crate::changes::{ChangeBus, DomainAudience, Envelope, EventId, Replay};
use crate::scope::{Scope, overlay_actor};

/// The keep-alive comment's cadence. Shorter than any idle timeout a proxy
/// ships with by default, so a quiet stream is never cut for silence.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// How long a resolved visibility answer is trusted before the next event
/// re-resolves it. What can leak inside the window is a domain name and a
/// path, never content.
pub const VISIBILITY_TTL: Duration = Duration::from_secs(10);

/// How many streams one account may hold open at once: a tab keeps one, so
/// this is a generous number of tabs and devices, and a runaway client is
/// refused long before it costs the instance anything.
pub const STREAMS_PER_ACCOUNT: usize = 32;

/// How many streams this process holds open at once, over every account and
/// the anonymous viewer (which counts toward this cap alone).
pub const STREAMS_TOTAL: usize = 4096;

/// What a refused stream is told to wait before it tries again.
pub const STREAMS_RETRY_AFTER_SECS: u64 = 30;

const LAST_EVENT_ID: &str = "last-event-id";

/// `GET /api/v1/events` - live changes to the knowledge, as a server-sent event
/// stream the web UI keeps open for the life of a tab.
///
/// Three events ride it. `engram` carries an `EngramChanged` body: one
/// engram, one kind (`added`, `modified`, `deleted`, `moved` with `from`),
/// its checksum and who changed it. `domain` carries a `DomainChanged`
/// body: a whole domain moved, refetch everything of it. `reset` carries
/// `{}` and no id: the client lost the thread and refetches everything. A
/// `: ping` comment arrives every fifteen seconds. `engram` and `domain`
/// frames carry `id: <epoch>:<seq>`; a request with `Last-Event-ID` is
/// caught up from the ring, or told `reset` when the id is another
/// process's or older than the ring holds. The stream is filtered by what
/// the session may read, and a draft's events reach its author alone.
#[utoipa::path(
    get,
    path = "/api/v1/events",
    tag = "events",
    summary = "Live changes to the knowledge, as a server-sent event stream.",
    description = "A `text/event-stream` the web UI keeps open once per tab. \
                   Frames: `id: <epoch>:<seq>` + `event: engram` + `data: \
                   <EngramChanged>`; `id` + `event: domain` + `data: \
                   <DomainChanged>`; `event: reset` + `data: {}` with no id \
                   (invalidate everything and keep the connection); and a \
                   `: ping` comment every fifteen seconds. `Last-Event-ID` \
                   replays the ring after that id, or answers one `reset` \
                   when the epoch is not this process's or the id is older \
                   than the ring's oldest entry. Every frame is filtered by \
                   what the session may read; a `draft_of` frame reaches \
                   the draft's owner alone. Served read-only. OpenAPI cannot \
                   type a stream, so the body below is described as text and \
                   the schemas `EngramChanged`, `DomainChanged` and \
                   `ChangeKind` carry the shapes.",
    params(
        (
            "Last-Event-ID" = Option<String>,
            Header,
            description = "The `id` of the last frame this client saw, as \
                           the browser's `EventSource` sends it on reconnect.",
            example = "1758542400:17",
        ),
    ),
    responses(
        (
            status = 200,
            description = "The stream. Headers: `Content-Type: \
                           text/event-stream`, `Cache-Control: no-cache`, \
                           `X-Accel-Buffering: no`.",
            body = String,
            content_type = "text/event-stream",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "Too many streams are open, for this account (32) or \
                           on this instance (4096). `Retry-After` says when \
                           to try again.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn stream_events(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = identity.scope();
    let slot = state.streams.acquire(&scope).ok_or_else(|| {
        ApiError::service_unavailable(
            "too many event streams are open for this account or on this instance; \
             close a tab or try again shortly",
        )
        .retry_after(STREAMS_RETRY_AFTER_SECS)
    })?;
    let hidden = state.engine.hidden_domains(&scope).await?;
    // Subscribe BEFORE the ring is read, so an event that lands between the
    // two is in the channel rather than lost; `last_sent` below drops the
    // one that is then in both.
    let receiver = state.engine.changes().subscribe();
    let (reset_pending, backlog) = match headers.get(LAST_EVENT_ID) {
        None => (false, Vec::new()),
        Some(value) => match catch_up(state.engine.changes(), value) {
            Replay::Reset => (true, Vec::new()),
            Replay::Events(events) => (false, events),
        },
    };
    let mut subscriber = Subscriber::new(state, headers, &identity, hidden, receiver, slot);
    subscriber.backlog = backlog.into();
    subscriber.reset_pending = reset_pending;
    let events = stream::unfold(subscriber, |mut subscriber| async move {
        subscriber
            .next()
            .await
            .map(|event| (Ok::<Event, Infallible>(event), subscriber))
    });
    let mut response = Sse::new(events)
        .keep_alive(KeepAlive::new().interval(HEARTBEAT).text("ping"))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}

/// What a request presenting `Last-Event-ID` is owed first. A value that is
/// not two integers around a colon is a `reset`, like an id from another
/// process, one older than the ring or one ahead of anything announced (the
/// bus answers those three): the client lost the thread either way, and
/// starting it live instead would hide what it missed.
fn catch_up(bus: &ChangeBus, value: &HeaderValue) -> Replay {
    let Some(id) = value.to_str().ok().and_then(EventId::parse) else {
        return Replay::Reset;
    };
    bus.replay_after(id)
}

/// The count of open streams, per account and in total, behind the caps
/// [`STREAMS_PER_ACCOUNT`] and [`STREAMS_TOTAL`]. One per [`RestState`],
/// shared by every clone of it; a [`StreamSlot`] gives its place back when
/// the stream it belongs to is dropped.
pub(crate) struct StreamSlots {
    per_account: usize,
    total: usize,
    open: Mutex<Open>,
    /// Each cap is logged the first time it refuses, and not again: a client
    /// retrying in a loop would otherwise fill the log.
    logged_account: AtomicBool,
    logged_total: AtomicBool,
}

#[derive(Default)]
struct Open {
    total: usize,
    by_account: HashMap<String, usize>,
}

impl Default for StreamSlots {
    fn default() -> StreamSlots {
        StreamSlots::new(STREAMS_PER_ACCOUNT, STREAMS_TOTAL)
    }
}

impl StreamSlots {
    pub(crate) fn new(per_account: usize, total: usize) -> StreamSlots {
        StreamSlots {
            per_account,
            total,
            open: Mutex::new(Open::default()),
            logged_account: AtomicBool::new(false),
            logged_total: AtomicBool::new(false),
        }
    }

    /// A place for one more stream of `scope`, or `None` when a cap is
    /// reached. An account counts toward both caps; the anonymous viewer and
    /// the machine owner toward the total alone.
    fn acquire(self: &Arc<Self>, scope: &Scope) -> Option<StreamSlot> {
        let account = match scope {
            Scope::User { account, .. } => Some(account.trim().to_lowercase()),
            Scope::Unrestricted | Scope::Anonymous => None,
        };
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if open.total >= self.total {
            if !self.logged_total.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "{} event streams are open, the most this instance serves at once; \
                     further streams are refused with 503 until some close",
                    self.total
                );
            }
            return None;
        }
        if let Some(account) = &account {
            let held = open.by_account.get(account).copied().unwrap_or(0);
            if held >= self.per_account {
                if !self.logged_account.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "account '{account}' holds {held} event streams, the most one account \
                         may; further streams for it are refused with 503 until some close"
                    );
                }
                return None;
            }
            *open.by_account.entry(account.clone()).or_default() += 1;
        }
        open.total += 1;
        Some(StreamSlot {
            slots: self.clone(),
            account,
        })
    }

    #[cfg(test)]
    fn open(&self) -> usize {
        self.open.lock().unwrap().total
    }
}

/// One open stream's place under the caps, given back on drop.
pub(crate) struct StreamSlot {
    slots: Arc<StreamSlots>,
    account: Option<String>,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut open = self.slots.open.lock().unwrap_or_else(|e| e.into_inner());
        open.total = open.total.saturating_sub(1);
        if let Some(account) = &self.account
            && let Some(held) = open.by_account.get_mut(account)
        {
            *held = held.saturating_sub(1);
            if *held == 0 {
                open.by_account.remove(account);
            }
        }
    }
}

/// One open stream: what it has still to say and whom it says it to.
struct Subscriber {
    state: RestState,
    /// The request's own headers, so its credentials (the session cookie or
    /// a header mode's identity) can be resolved again at each refresh.
    headers: HeaderMap,
    /// The account the stream was opened as, folded; `None` for the
    /// anonymous viewer. A refresh that no longer resolves to it ends the
    /// stream.
    account: Option<String>,
    scope: Scope,
    /// Whose drafts this session may hear about: its own overlay actor.
    overlay: Option<String>,
    /// `None` for a scope that hides nothing; the hidden names otherwise.
    hidden: Option<HashSet<String>>,
    resolved_at: Instant,
    /// When the account behind the stream was last re-checked. An idle
    /// stream re-checks it every [`VISIBILITY_TTL`] on its own, so a logout
    /// or a revocation ends it (and frees its slot) without waiting for an
    /// event.
    checked_at: Instant,
    /// Set by an event that carried a captured audience (the change it
    /// announced moved the privacy records), by a lag (such an event may be
    /// among the ones lost) and by a failed re-resolution: `hidden` is
    /// re-resolved before the next ordinary check whatever its age.
    stale: bool,
    receiver: broadcast::Receiver<Envelope>,
    /// The replay, drained ahead of the live channel.
    backlog: VecDeque<Envelope>,
    reset_pending: bool,
    /// The newest id written, so a replayed entry that also arrived live is
    /// written once.
    last_sent: Option<EventId>,
    shutdown: Option<watch::Receiver<bool>>,
    /// This stream's place under the caps, held for as long as it is open.
    _slot: StreamSlot,
}

/// What one envelope comes to for this session.
enum Step {
    Send(Event),
    Skip,
    /// The session behind the stream is gone: end it.
    End,
}

/// What the live half of [`Subscriber::next`] woke up to.
// One value on the stack per wake-up; boxing the envelope would allocate for
// every event to save nothing.
#[allow(clippy::large_enum_variant)]
enum Woke {
    Shutdown,
    Recheck,
    Received(Result<Envelope, RecvError>),
}

/// Resolves at `deadline`; never, for `None`.
async fn at(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => futures::future::pending().await,
    }
}

/// Whether this session may hear one event.
enum Seen {
    Yes,
    No,
    /// The session behind the stream is gone.
    Gone,
}

/// What a refresh of who is listening found.
enum Refreshed {
    /// Still the same caller, now with this scope.
    Same(Scope),
    /// The session was logged out or revoked, or the account is gone or
    /// disabled.
    Gone,
    /// The store could not answer; nothing is known.
    Unknown,
}

impl Subscriber {
    fn new(
        state: RestState,
        headers: HeaderMap,
        identity: &Identity,
        hidden: Option<HashSet<String>>,
        receiver: broadcast::Receiver<Envelope>,
        slot: StreamSlot,
    ) -> Subscriber {
        let scope = identity.scope();
        Subscriber {
            shutdown: state.shutdown.clone(),
            state,
            headers,
            account: identity
                .user
                .as_ref()
                .map(|user| user.name.trim().to_lowercase()),
            overlay: overlay_actor(&scope),
            scope,
            hidden,
            resolved_at: Instant::now(),
            checked_at: Instant::now(),
            stale: false,
            receiver,
            backlog: VecDeque::new(),
            reset_pending: false,
            last_sent: None,
            _slot: slot,
        }
    }

    /// The next frame, or `None` when the stream is over.
    async fn next(&mut self) -> Option<Event> {
        loop {
            if self.reset_pending {
                self.reset_pending = false;
                return Some(reset_frame());
            }
            let envelope = match self.backlog.pop_front() {
                Some(envelope) => envelope,
                None => {
                    // Selected into a value first, so the arms below may
                    // borrow `self` again.
                    let recheck_at = self
                        .account
                        .is_some()
                        .then(|| self.checked_at + VISIBILITY_TTL);
                    let woke = tokio::select! {
                        biased;
                        _ = closed(&mut self.shutdown) => Woke::Shutdown,
                        _ = at(recheck_at) => Woke::Recheck,
                        received = self.receiver.recv() => Woke::Received(received),
                    };
                    match woke {
                        Woke::Shutdown | Woke::Received(Err(RecvError::Closed)) => return None,
                        Woke::Recheck => {
                            if self.recheck().await {
                                continue;
                            }
                            return None;
                        }
                        Woke::Received(Err(RecvError::Lagged(_))) => {
                            // A rename's captured frame may be among the
                            // lost ones, so the cache is not trusted past it.
                            self.stale = true;
                            return Some(reset_frame());
                        }
                        Woke::Received(Ok(envelope)) => envelope,
                    }
                }
            };
            match self.frame(&envelope).await {
                Step::Send(event) => return Some(event),
                Step::Skip => continue,
                Step::End => return None,
            }
        }
    }

    /// What one envelope comes to: its frame, nothing (this session may not
    /// hear it, or already did), or the end of the stream.
    async fn frame(&mut self, envelope: &Envelope) -> Step {
        if let Some(last) = self.last_sent
            && envelope.id <= last
        {
            return Step::Skip;
        }
        self.last_sent = Some(envelope.id);
        if let Some(owner) = envelope.change.draft_of()
            && self.overlay.as_deref() != Some(owner)
        {
            return Step::Skip;
        }
        match self
            .may_see(envelope.change.domain(), envelope.change.audience())
            .await
        {
            Seen::Yes => {}
            Seen::No => return Step::Skip,
            Seen::Gone => return Step::End,
        }
        let Ok(data) = envelope.change.data() else {
            return Step::Skip;
        };
        Step::Send(
            Event::default()
                .id(envelope.id.to_string())
                .event(envelope.change.name())
                .data(data),
        )
    }

    /// Whether `domain` is visible to this session, or [`Seen::Gone`] when
    /// the session behind the stream is gone.
    ///
    /// `audience`, when `Some`, is the snapshot the engine itself resolved
    /// at the moment of a change that takes the name out of the privacy
    /// records (a domain's rename, under its old name, and its removal;
    /// ruled 2026-09-27; the engram events under that name carry it too,
    /// since a path leaks the name as surely as the domain event does). A frame with none of its own is checked against the
    /// capture the bus holds for its domain right now, since it may have sat
    /// in the channel from before the capture began. A captured audience is
    /// checked directly, against this session's own identity, and neither
    /// this subscriber's cache nor a fresh `hidden_domains` call is consulted
    /// for it, since both could answer from a registry the change itself is
    /// emptying, or, symmetrically, still answer "member" a moment after that
    /// access was revoked. The same event marks the cache stale, because the
    /// records it was built from have just moved.
    ///
    /// Every other event keeps the ordinary check, re-resolved after
    /// [`VISIBILITY_TTL`] or once stale. A refresh that is due re-reads who is
    /// listening first (see [`Subscriber::refresh_identity`]). A refresh that
    /// fails drops the event and is tried again on the next one.
    async fn may_see(&mut self, domain: &str, audience: Option<&DomainAudience>) -> Seen {
        let captured;
        let audience = match audience {
            Some(audience) => Some(audience),
            None => {
                captured = self.state.engine.changes().captured_audience(domain);
                captured.as_ref()
            }
        };
        let due = self.resolved_at.elapsed() > VISIBILITY_TTL;
        if due || (self.stale && audience.is_none()) {
            match self.refresh().await {
                Refreshed::Gone => return Seen::Gone,
                Refreshed::Unknown => return Seen::No,
                Refreshed::Same(_) => {}
            }
        }
        let admitted = match audience {
            Some(audience) => {
                self.stale = true;
                audience_admits(audience, &self.scope)
            }
            None => match &self.hidden {
                None => true,
                Some(hidden) => !hidden.contains(domain),
            },
        };
        if admitted { Seen::Yes } else { Seen::No }
    }

    /// The idle re-check: whether the account behind the stream still holds
    /// it. `false` ends the stream. A changed role marks the cache stale, so
    /// the next event re-resolves what it may see; a store that cannot
    /// answer is asked again at the next tick.
    async fn recheck(&mut self) -> bool {
        let refreshed = self.refresh_identity().await;
        self.checked_at = Instant::now();
        match refreshed {
            Refreshed::Gone => false,
            Refreshed::Same(scope) => {
                if scope != self.scope {
                    self.scope = scope;
                    self.stale = true;
                }
                true
            }
            Refreshed::Unknown => true,
        }
    }

    /// Re-read who is listening, then what they may see. On success the
    /// scope and the hidden set are current and the cache is fresh; on
    /// failure the cache stays stale so the next event tries again.
    async fn refresh(&mut self) -> Refreshed {
        let refreshed = self.refresh_identity().await;
        self.checked_at = Instant::now();
        let scope = match refreshed {
            Refreshed::Same(scope) => scope,
            other => {
                self.stale = true;
                return other;
            }
        };
        match self.state.engine.hidden_domains(&scope).await {
            Ok(hidden) => {
                self.hidden = hidden;
                self.scope = scope.clone();
                self.stale = false;
                self.resolved_at = Instant::now();
                Refreshed::Same(scope)
            }
            Err(_) => {
                self.scope = scope;
                self.stale = true;
                Refreshed::Unknown
            }
        }
    }

    /// Resolve the request's credentials again, reading only (see
    /// `auth::resolve_quiet`: a refresh writes nothing to the accounts
    /// store, so many streams re-checking at once never queue behind its
    /// writes or stamp their owners as just seen). An account stream is
    /// `Gone` when they no
    /// longer resolve to that account (logged out, session revoked or
    /// expired, account deleted or disabled) and `Same` with a scope rebuilt
    /// from the stored role otherwise, so a demoted admin loses admin
    /// visibility here. The anonymous viewer has no credentials to lose and
    /// keeps its scope. A store that cannot answer is `Unknown`.
    async fn refresh_identity(&self) -> Refreshed {
        let Some(account) = &self.account else {
            return Refreshed::Same(self.scope.clone());
        };
        match super::auth::resolve_quiet(&self.state, &self.headers, account).await {
            Ok(Some(user)) if user.name.trim().to_lowercase() == *account => {
                Refreshed::Same(Scope::User {
                    account: user.name,
                    admin: user.role == crate::Role::Admin,
                })
            }
            Ok(_) => Refreshed::Gone,
            Err(e) if e.status == StatusCode::UNAUTHORIZED || e.status == StatusCode::FORBIDDEN => {
                Refreshed::Gone
            }
            Err(_) => Refreshed::Unknown,
        }
    }
}

/// Whether a captured audience admits `scope`. The machine owner always
/// reads; an account reads when it is an instance admin or when its login
/// name, folded the way the store folds the names it holds (trimmed and
/// lowercased), is in the set; the anonymous viewer reads only a domain
/// that was shared with everyone.
fn audience_admits(audience: &DomainAudience, scope: &Scope) -> bool {
    match (audience, scope) {
        (DomainAudience::Everyone, _) | (_, Scope::Unrestricted) => true,
        (DomainAudience::Accounts(accounts), Scope::User { account, admin }) => {
            *admin || accounts.contains(&account.trim().to_lowercase())
        }
        (DomainAudience::Accounts(_), Scope::Anonymous) => false,
    }
}

fn reset_frame() -> Event {
    Event::default().event("reset").data("{}")
}

/// Resolves when the daemon shuts down; never, on a router with no daemon.
async fn closed(shutdown: &mut Option<watch::Receiver<bool>>) {
    match shutdown {
        Some(rx) => {
            // A sender dropped without flipping is a daemon gone too.
            let _ = rx.wait_for(|flipped| *flipped).await;
        }
        None => futures::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::{Change, ChangeKind, EngramChanged};
    use crate::engine::Engine;

    /// A state over an engine with no resolver installed, which hides
    /// nothing, and a fresh accounts store.
    async fn state() -> (tempfile::TempDir, RestState) {
        let dir = tempfile::tempdir().unwrap();
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            crystalline_core::config::GlobalConfig::default(),
            None,
            None,
        ));
        let auth = Arc::new(
            crate::AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        (dir, RestState::new(engine, auth, &[]).unwrap())
    }

    fn modified(line: &str) -> Change {
        Change::Engram(EngramChanged {
            domain: "eng".to_string(),
            permalink: "alpha".to_string(),
            path: "alpha.md".to_string(),
            kind: ChangeKind::Modified,
            from: None,
            checksum: Some(line.to_string()),
            actor: None,
            draft_of: None,
            audience: None,
        })
    }

    fn anonymous() -> Identity {
        Identity {
            user: None,
            csrf: None,
            anonymous: true,
        }
    }

    fn account(name: &str) -> Identity {
        Identity {
            user: Some(crate::User {
                name: name.to_string(),
                display: name.to_string(),
                email: None,
                role: crate::Role::Viewer,
                disabled: false,
                last_seen: None,
            }),
            csrf: None,
            anonymous: false,
        }
    }

    /// A subscriber of `state` as `identity`, subscribed now, with `hidden`
    /// as its cache and every ring entry after `after` as its backlog.
    fn subscriber(
        state: &RestState,
        identity: &Identity,
        hidden: Option<HashSet<String>>,
        receiver: broadcast::Receiver<Envelope>,
        after: Option<EventId>,
    ) -> Subscriber {
        let slot = state.streams.acquire(&identity.scope()).unwrap();
        let mut subscriber = Subscriber::new(
            state.clone(),
            HeaderMap::new(),
            identity,
            hidden,
            receiver,
            slot,
        );
        if let Some(after) = after {
            match state.engine.changes().replay_after(after) {
                Replay::Events(events) => subscriber.backlog = events.into(),
                Replay::Reset => panic!("the ring holds the event"),
            }
        }
        subscriber
    }

    fn before(id: EventId) -> EventId {
        EventId {
            epoch: id.epoch,
            seq: id.seq - 1,
        }
    }

    /// Whether the subscriber writes a frame within a moment.
    async fn hears(subscriber: &mut Subscriber) -> bool {
        tokio::time::timeout(Duration::from_millis(200), subscriber.next())
            .await
            .is_ok_and(|event| event.is_some())
    }

    #[tokio::test]
    async fn an_event_in_both_the_replay_and_the_channel_is_written_once() {
        // Catches the `last_sent` check dropped: the handler subscribes
        // before it reads the ring, so an event announced between the two
        // is in both, and the client would hear it twice. Built here, where
        // the overlap can be arranged, since over the wire it is a race.
        let (_dir, state) = state().await;
        let bus = state.engine.changes();
        let receiver = bus.subscribe();
        let first = bus.announce(modified("1")).unwrap();
        let mut subscriber = subscriber(&state, &anonymous(), None, receiver, Some(before(first)));
        assert!(hears(&mut subscriber).await, "the replayed frame");
        assert!(
            !hears(&mut subscriber).await,
            "the live copy of the same event is not written again"
        );
        bus.announce(modified("2")).unwrap();
        assert!(hears(&mut subscriber).await, "the next event is");
    }

    fn user(account: &str, admin: bool) -> Scope {
        Scope::User {
            account: account.to_string(),
            admin,
        }
    }

    fn members(names: &[&str]) -> DomainAudience {
        DomainAudience::Accounts(names.iter().map(|n| n.to_string()).collect())
    }

    #[tokio::test]
    async fn a_replayed_engram_event_with_a_captured_audience_is_checked_against_it() {
        // Catches a check that honoured the captured audience on `domain`
        // events alone, or on live events alone: the bus stamps it on engram
        // events under a renamed or removed name and on the ring entries
        // already held for it, and this session's cache (which
        // hides nothing) would let the path through.
        let (_dir, state) = state().await;
        let bus = state.engine.changes();
        let receiver = bus.subscribe();
        let mut stamped = modified("1");
        if let Change::Engram(change) = &mut stamped {
            change.audience = Some(members(&["ada"]));
        }
        let first = bus.announce(stamped).unwrap();
        let mut outsider = subscriber(
            &state,
            &account("vera"),
            Some(HashSet::new()),
            receiver,
            Some(before(first)),
        );
        assert!(
            !hears(&mut outsider).await,
            "neither the replayed nor the live copy reaches vera"
        );
    }

    #[tokio::test]
    async fn a_frame_queued_before_a_capture_is_checked_against_the_capture() {
        // Catches the delivery-time lookup dropped: an event
        // announced before the capture began sits in the channel with no
        // audience of its own, and the ring re-stamp cannot reach it.
        let (_dir, state) = state().await;
        let bus = state.engine.changes();
        let receiver = bus.subscribe();
        bus.announce(modified("1")).unwrap();
        bus.capture("eng", members(&["ada"]));
        // The anonymous viewer, whose refresh needs no credentials: an
        // account here would be refreshed after the captured frame marked
        // the cache stale, find nothing behind this bare request and end.
        let mut outsider = subscriber(&state, &anonymous(), Some(HashSet::new()), receiver, None);
        assert!(
            !hears(&mut outsider).await,
            "the anonymous viewer is outside the capture"
        );
        bus.release("eng");
        bus.announce(modified("2")).unwrap();
        assert!(
            hears(&mut outsider).await,
            "after the release the ordinary check is back"
        );
    }

    #[tokio::test]
    async fn the_cache_is_re_resolved_once_it_is_older_than_the_ttl() {
        // Catches the age half of the refresh dropped: "a revocation lands
        // within ten seconds" and its converse (a new member starts hearing)
        // rest on it. The engine hides nothing, so a re-resolution clears the stale
        // `eng` from the cache, and a fresh cache is trusted as it is.
        let (_dir, state) = state().await;
        let bus = state.engine.changes();
        let hidden = || Some(HashSet::from(["eng".to_string()]));

        let mut fresh = subscriber(&state, &anonymous(), hidden(), bus.subscribe(), None);
        let mut aged = subscriber(&state, &anonymous(), hidden(), bus.subscribe(), None);
        aged.resolved_at = Instant::now() - (VISIBILITY_TTL + Duration::from_secs(1));
        bus.announce(modified("1")).unwrap();
        assert!(!hears(&mut fresh).await, "a fresh cache is trusted");
        assert!(hears(&mut aged).await, "an aged cache is re-resolved first");
    }

    #[tokio::test]
    async fn a_lag_marks_the_cache_stale() {
        // Catches a lag that trusted the cache afterwards: the
        // lost events may include a rename's captured frame, so the next
        // frame is checked against the records as they are now.
        let (_dir, state) = state().await;
        let bus = state.engine.changes();
        let mut subscriber = subscriber(
            &state,
            &anonymous(),
            Some(HashSet::from(["eng".to_string()])),
            bus.subscribe(),
            None,
        );
        for i in 0..(crate::changes::CHANNEL_CAPACITY + 1) {
            bus.announce(modified(&i.to_string())).unwrap();
        }
        let reset = subscriber.next().await.unwrap();
        assert!(format!("{reset:?}").contains("reset"), "{reset:?}");
        assert!(subscriber.stale, "the cache is not trusted past a lag");
        assert!(
            hears(&mut subscriber).await,
            "re-resolved, `eng` is visible"
        );
    }

    #[tokio::test]
    async fn an_account_whose_session_no_longer_resolves_ends_its_stream() {
        // Catches the identity frozen at connect: the request
        // carries no credential the store knows, so the refresh finds no
        // account behind it and the stream ends instead of writing.
        let (_dir, state) = state().await;
        let bus = state.engine.changes();
        let mut gone = subscriber(&state, &account("ada"), None, bus.subscribe(), None);
        gone.stale = true;
        bus.announce(modified("1")).unwrap();
        let next = tokio::time::timeout(Duration::from_secs(2), gone.next()).await;
        assert!(matches!(next, Ok(None)), "the stream ends: {next:?}");
    }

    #[test]
    fn the_stream_caps_count_per_account_and_in_total_and_give_back_on_drop() {
        // Catches either cap not enforced, the anonymous viewer counted
        // against an account, and a slot never given back.
        let slots = Arc::new(StreamSlots::new(2, 3));
        let a1 = slots.acquire(&user("ada", false)).unwrap();
        let _a2 = slots.acquire(&user(" ADA ", false)).unwrap();
        assert!(slots.acquire(&user("ada", false)).is_none(), "per account");
        let _v = slots.acquire(&user("vera", false)).unwrap();
        assert!(slots.acquire(&Scope::Anonymous).is_none(), "in total");
        drop(a1);
        assert_eq!(slots.open(), 2);
        let _anon = slots.acquire(&Scope::Anonymous).unwrap();
        assert!(
            slots.acquire(&user("zoe", false)).is_none(),
            "in total again"
        );
    }

    #[test]
    fn a_captured_audience_admits_its_members_by_the_folded_name() {
        // Catches a check that compared the raw spelling: the set holds the
        // store's folded names, and a scope spelled otherwise is the same
        // account.
        let lab = members(&["ada"]);
        assert!(audience_admits(&lab, &user("ada", false)));
        assert!(audience_admits(&lab, &user(" Ada ", false)));
        assert!(!audience_admits(&lab, &user("vera", false)));
    }

    #[test]
    fn an_admin_the_machine_owner_and_everyone_read_and_the_anonymous_viewer_does_not() {
        // Catches an admin flag ignored (the set comes from the stored role
        // and can miss what the surface asserted), the owner held back, and
        // the anonymous viewer let into a private domain.
        let lab = members(&["ada"]);
        assert!(audience_admits(&lab, &user("root", true)));
        assert!(audience_admits(&lab, &Scope::Unrestricted));
        assert!(!audience_admits(&lab, &Scope::Anonymous));
        assert!(audience_admits(
            &DomainAudience::Everyone,
            &Scope::Anonymous
        ));
        assert!(audience_admits(
            &DomainAudience::Everyone,
            &user("vera", false)
        ));
    }
}
