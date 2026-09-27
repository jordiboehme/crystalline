//! `GET /events`: the server-sent event stream a tab keeps open so the page
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
//! altering (Section J (k), ruled 2026-09-27). Such an event also marks the
//! cache stale, so the frame that follows it (a rename's new name) is checked
//! against the records as they are after the change rather than a cache from
//! before it. A draft event reaches the draft's owner alone. A client that
//! presents the last id it saw is caught up from the engine's ring or told to
//! `reset`; one that falls behind the channel is told to `reset` too. Every
//! fifteen seconds a comment keeps the connection alive through proxies, and
//! the daemon's shutdown ends the stream so the drain never waits on a
//! browser.

use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use super::auth::Identity;
use super::{ApiError, ProblemDetail, RestState};
use crate::changes::{ChangeBus, DomainAudience, Envelope, EventId, Replay};
use crate::engine::Engine;
use crate::scope::{Scope, overlay_actor};

/// The keep-alive comment's cadence. Shorter than any idle timeout a proxy
/// ships with by default, so a quiet stream is never cut for silence.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// How long a resolved visibility answer is trusted before the next event
/// re-resolves it. What can leak inside the window is a domain name and a
/// path, never content.
pub const VISIBILITY_TTL: Duration = Duration::from_secs(10);

const LAST_EVENT_ID: &str = "last-event-id";

/// `GET /events` - live changes to the knowledge, as a server-sent event
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
    ),
)]
pub async fn stream_events(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = identity.scope();
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
    let subscriber = Subscriber {
        engine: state.engine.clone(),
        overlay: overlay_actor(&scope),
        scope,
        hidden,
        resolved_at: Instant::now(),
        stale: false,
        receiver,
        backlog: backlog.into(),
        reset_pending,
        last_sent: None,
        shutdown: state.shutdown.clone(),
    };
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

/// One open stream: what it has still to say and whom it says it to.
struct Subscriber {
    engine: Arc<Engine>,
    scope: Scope,
    /// Whose drafts this session may hear about: its own overlay actor.
    overlay: Option<String>,
    /// `None` for a scope that hides nothing; the hidden names otherwise.
    hidden: Option<HashSet<String>>,
    resolved_at: Instant,
    /// Set by an event that carried a captured audience: the change it
    /// announced moved the privacy records, so `hidden` is re-resolved
    /// before the next ordinary check whatever its age.
    stale: bool,
    receiver: broadcast::Receiver<Envelope>,
    /// The replay, drained ahead of the live channel.
    backlog: VecDeque<Envelope>,
    reset_pending: bool,
    /// The newest id written, so a replayed entry that also arrived live is
    /// written once.
    last_sent: Option<EventId>,
    shutdown: Option<watch::Receiver<bool>>,
}

impl Subscriber {
    /// The next frame, or `None` when the stream is over.
    async fn next(&mut self) -> Option<Event> {
        loop {
            if self.reset_pending {
                self.reset_pending = false;
                return Some(reset_frame());
            }
            if let Some(envelope) = self.backlog.pop_front() {
                if let Some(event) = self.frame(&envelope).await {
                    return Some(event);
                }
                continue;
            }
            // `None` is the daemon shutting down. Selected into a value
            // first, so the branch below may borrow `self` again.
            let received = tokio::select! {
                biased;
                _ = closed(&mut self.shutdown) => None,
                received = self.receiver.recv() => Some(received),
            };
            match received {
                None | Some(Err(RecvError::Closed)) => return None,
                Some(Err(RecvError::Lagged(_))) => return Some(reset_frame()),
                Some(Ok(envelope)) => {
                    if let Some(event) = self.frame(&envelope).await {
                        return Some(event);
                    }
                }
            }
        }
    }

    /// The frame for one envelope, or `None` when this session may not hear
    /// it or already did.
    async fn frame(&mut self, envelope: &Envelope) -> Option<Event> {
        if let Some(last) = self.last_sent
            && envelope.id <= last
        {
            return None;
        }
        self.last_sent = Some(envelope.id);
        if let Some(owner) = envelope.change.draft_of()
            && self.overlay.as_deref() != Some(owner)
        {
            return None;
        }
        if !self
            .may_see(envelope.change.domain(), envelope.change.audience())
            .await
        {
            return None;
        }
        let data = envelope.change.data().ok()?;
        Some(
            Event::default()
                .id(envelope.id.to_string())
                .event(envelope.change.name())
                .data(data),
        )
    }

    /// Whether `domain` is visible to this session. `audience`, when
    /// `Some`, is the snapshot the engine itself resolved at the moment of
    /// a change that takes the name out of the privacy records (a domain's
    /// rename, under its old name, and its removal; Section J (k), ruled
    /// 2026-09-27): it is checked directly, against this session's own
    /// identity, and neither this subscriber's cache nor a fresh
    /// `hidden_domains` call is consulted for it, since both could answer
    /// from a registry the change itself is emptying, or, symmetrically,
    /// still answer "member" a moment after that access was revoked. The
    /// same event marks the cache stale, because the records it was built
    /// from have just moved. Every other event carries `None` and keeps the
    /// ordinary check, re-resolved after [`VISIBILITY_TTL`] or once stale; a
    /// failed re-resolution keeps the last answer.
    async fn may_see(&mut self, domain: &str, audience: Option<&DomainAudience>) -> bool {
        if let Some(audience) = audience {
            self.stale = true;
            return audience_admits(audience, &self.scope);
        }
        if self.stale || self.resolved_at.elapsed() > VISIBILITY_TTL {
            if let Ok(hidden) = self.engine.hidden_domains(&self.scope).await {
                self.hidden = hidden;
                self.stale = false;
            }
            self.resolved_at = Instant::now();
        }
        match &self.hidden {
            None => true,
            Some(hidden) => !hidden.contains(domain),
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

    /// An engine with no resolver installed, which hides nothing.
    async fn engine() -> Arc<Engine> {
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            crystalline_core::config::GlobalConfig::default(),
            None,
            None,
        ))
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

    #[tokio::test]
    async fn an_event_in_both_the_replay_and_the_channel_is_written_once() {
        // Catches the `last_sent` check dropped: the handler subscribes
        // before it reads the ring, so an event announced between the two
        // is in both, and the client would hear it twice. Built here, where
        // the overlap can be arranged, since over the wire it is a race.
        let engine = engine().await;
        let receiver = engine.changes().subscribe();
        let first = engine.changes().announce(modified("1")).unwrap();
        let backlog = match engine.changes().replay_after(EventId {
            epoch: first.epoch,
            seq: first.seq - 1,
        }) {
            Replay::Events(events) => events,
            Replay::Reset => panic!("the ring holds the event"),
        };
        let mut subscriber = Subscriber {
            engine: engine.clone(),
            overlay: None,
            scope: Scope::Unrestricted,
            hidden: None,
            resolved_at: Instant::now(),
            stale: false,
            receiver,
            backlog: backlog.into(),
            reset_pending: false,
            last_sent: None,
            shutdown: None,
        };
        assert!(subscriber.next().await.is_some(), "the replayed frame");
        let again = tokio::time::timeout(Duration::from_millis(200), subscriber.next()).await;
        assert!(
            again.is_err(),
            "the live copy of the same event is not written again"
        );
        engine.changes().announce(modified("2")).unwrap();
        assert!(subscriber.next().await.is_some(), "the next event is");
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
        // already held for it (ruling K2), and this session's cache (which
        // hides nothing) would let the path through.
        let engine = engine().await;
        let receiver = engine.changes().subscribe();
        let mut stamped = modified("1");
        if let Change::Engram(change) = &mut stamped {
            change.audience = Some(members(&["ada"]));
        }
        let first = engine.changes().announce(stamped).unwrap();
        let backlog = match engine.changes().replay_after(EventId {
            epoch: first.epoch,
            seq: first.seq - 1,
        }) {
            Replay::Events(events) => events,
            Replay::Reset => panic!("the ring holds the event"),
        };
        let mut outsider = Subscriber {
            engine: engine.clone(),
            overlay: Some("vera".to_string()),
            scope: user("vera", false),
            hidden: Some(HashSet::new()),
            resolved_at: Instant::now(),
            stale: false,
            receiver,
            backlog: backlog.into(),
            reset_pending: false,
            last_sent: None,
            shutdown: None,
        };
        let heard = tokio::time::timeout(Duration::from_millis(200), outsider.next()).await;
        assert!(
            heard.is_err(),
            "neither the replayed nor the live copy reaches vera"
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
