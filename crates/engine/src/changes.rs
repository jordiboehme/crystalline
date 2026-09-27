//! The change bus: one announcement per change the store just committed, for
//! whoever is listening, and the ring beside it that lets a listener who
//! stepped away catch up.
//!
//! It lives on the shared [`crate::engine::Engine`] for the reason
//! [`crate::subscribers`] does: a change lands through the watcher, a verb,
//! a room's save, a pull or a discard, and the only object every one of those
//! shares with the route that streams them is the engine. Sending never
//! awaits and never blocks: the mutex is held to push and pop the ring and
//! nothing else, and a send nobody listens to is not an error. A daemonless
//! engine carries a bus with no subscriber and pays one `send` per change.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

/// How many events the ring keeps for replay. A tab away for a larger burst
/// gets a `reset` (one refetch of every active query), which is the soft
/// failure this whole design prefers.
pub const RING_CAPACITY: usize = 512;

/// The broadcast channel's capacity per subscriber; a subscriber further
/// behind than this is answered `Lagged`, which the route turns into `reset`.
pub const CHANNEL_CAPACITY: usize = 1024;

/// Above this many announced paths a sync report becomes one `DomainChanged`.
pub const COLLAPSE_THRESHOLD: usize = 32;

/// An `engram` event equal to the previous one in domain, path, kind and
/// checksum inside this window is dropped: a platform whose mtime granularity
/// makes the watcher re-announce an engine write costs nobody a refetch.
pub const DUPLICATE_WINDOW: Duration = Duration::from_secs(2);

/// What happened to the engram: the four things the sync engine counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Moved,
}

impl From<crystalline_index::PathChangeKind> for ChangeKind {
    fn from(kind: crystalline_index::PathChangeKind) -> ChangeKind {
        use crystalline_index::PathChangeKind as K;
        match kind {
            K::Added => ChangeKind::Added,
            K::Modified => ChangeKind::Modified,
            K::Deleted => ChangeKind::Deleted,
            K::Moved => ChangeKind::Moved,
        }
    }
}

/// Where a moved engram came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MovedFrom {
    /// Domain-relative, forward slashes, `.md`.
    pub path: String,
    pub permalink: String,
}

/// One engram changed, as every subscriber hears it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EngramChanged {
    pub domain: String,
    /// After the change; for `deleted`, what the row had before it went.
    pub permalink: String,
    /// Domain-relative, forward slashes, `.md`; after the change.
    pub path: String,
    pub kind: ChangeKind,
    /// `moved` only.
    pub from: Option<MovedFrom>,
    /// Lowercase hex SHA-256 of the new content, the value every write
    /// receipt reports as `checksum`. Absent for `deleted`.
    pub checksum: Option<String>,
    /// Who made the change, as a display label. Null for a change the watcher
    /// found on disk and for a write nobody signed.
    pub actor: Option<String>,
    /// Set when the change landed in this actor's draft overlay. Delivered to
    /// that actor's own sessions and to nobody else.
    pub draft_of: Option<String>,
    /// Who may hear this event, when that was captured for its domain's name:
    /// set by the bus, never by a feed point, while the domain is being
    /// renamed or removed (ruling K2, 2026-09-27: a path under the old name
    /// leaks the name as surely as the domain event does). `None` keeps the
    /// ordinary per-session check. Never on the wire.
    #[serde(skip)]
    pub audience: Option<DomainAudience>,
}

/// The identities that may read a domain, captured once by the engine at the
/// moment of a change that takes the domain's name out of the privacy records
/// (a removal, and the old name of a rename; ruled 2026-09-27).
///
/// `Everyone` when the domain is not private. `Accounts` when it is, naming
/// every account that may read it: its owner, every member at any level and
/// every instance admin by the role the accounts store records, disabled
/// accounts left out. The names are the store's folded spelling (trimmed and
/// lowercased), so a check folds `Scope::User.account` the same way, and a
/// scope that carries `admin: true` reads too, since the surface's own admin
/// flag outranks the stored role. The machine owner always reads, so it is
/// never in the set. When the engine cannot read the records (no resolver
/// installed yet, or a read error) it captures `Accounts` with nobody in it:
/// the owner still hears the event, nobody else does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomainAudience {
    Everyone,
    Accounts(HashSet<String>),
}

/// A whole domain moved: refetch everything of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DomainChanged {
    pub domain: String,
    pub actor: Option<String>,
    /// `Some`, resolved eagerly, only for a rename's old name and a removal:
    /// that name's privacy record leaves with the change, so the route must
    /// filter against this snapshot rather than a session's own lazy check.
    /// `None` keeps the ordinary per-session check for every other domain
    /// event. Never on the wire.
    #[serde(skip)]
    pub audience: Option<DomainAudience>,
}

/// One announcement on the bus. The wire's third event, `reset`, is the
/// route's own and never rides the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Engram(EngramChanged),
    Domain(DomainChanged),
}

impl Change {
    /// The SSE `event` field.
    pub fn name(&self) -> &'static str {
        match self {
            Change::Engram(_) => "engram",
            Change::Domain(_) => "domain",
        }
    }

    pub fn domain(&self) -> &str {
        match self {
            Change::Engram(change) => &change.domain,
            Change::Domain(change) => &change.domain,
        }
    }

    /// Stamp a captured audience onto a change that carries none of its own.
    fn stamp(&mut self, audience: &DomainAudience) {
        let slot = match self {
            Change::Engram(change) => &mut change.audience,
            Change::Domain(change) => &mut change.audience,
        };
        if slot.is_none() {
            *slot = Some(audience.clone());
        }
    }

    pub fn draft_of(&self) -> Option<&str> {
        match self {
            Change::Engram(change) => change.draft_of.as_deref(),
            Change::Domain(_) => None,
        }
    }

    /// The audience captured at the moment of the change, when there is one.
    /// `None` keeps the ordinary per-session visibility check.
    pub fn audience(&self) -> Option<&DomainAudience> {
        match self {
            Change::Engram(change) => change.audience.as_ref(),
            Change::Domain(change) => change.audience.as_ref(),
        }
    }

    /// The SSE `data` field: the inner struct alone, since the event name
    /// carries the tag.
    pub fn data(&self) -> serde_json::Result<String> {
        match self {
            Change::Engram(change) => serde_json::to_string(change),
            Change::Domain(change) => serde_json::to_string(change),
        }
    }
}

/// `<epoch>:<seq>`: the daemon's start instant in Unix seconds and the bus's
/// counter from 1. A restart is a new epoch, and a reconnect across one gets
/// a `reset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventId {
    pub epoch: u64,
    pub seq: u64,
}

impl EventId {
    /// Parse a `Last-Event-ID` value; `None` for anything that is not two
    /// integers around a colon.
    pub fn parse(text: &str) -> Option<EventId> {
        let (epoch, seq) = text.trim().split_once(':')?;
        Some(EventId {
            epoch: epoch.parse().ok()?,
            seq: seq.parse().ok()?,
        })
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.seq)
    }
}

/// A change with its id, as the ring keeps it and the channel carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub id: EventId,
    pub change: Change,
}

/// What a subscriber presenting a last id is owed first.
#[derive(Debug, PartialEq, Eq)]
pub enum Replay {
    /// The id is from another process or older than the ring holds.
    Reset,
    /// Every ring entry after the id, in order; possibly none.
    Events(Vec<Envelope>),
}

struct Ring {
    entries: VecDeque<Envelope>,
    /// The last `engram` announcement and when, for the duplicate window.
    last_engram: Option<(EngramChanged, Instant)>,
    /// The audiences captured for domain names being renamed or removed,
    /// stamped onto every event under that name until the capture ends.
    captured: HashMap<String, DomainAudience>,
}

/// The bus itself. See the module doc for the discipline on its mutex.
pub struct ChangeBus {
    epoch: u64,
    seq: AtomicU64,
    tx: broadcast::Sender<Envelope>,
    ring: Mutex<Ring>,
}

impl Default for ChangeBus {
    fn default() -> ChangeBus {
        ChangeBus::new()
    }
}

impl ChangeBus {
    pub fn new() -> ChangeBus {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        ChangeBus {
            epoch,
            seq: AtomicU64::new(0),
            tx,
            ring: Mutex::new(Ring {
                entries: VecDeque::with_capacity(RING_CAPACITY),
                last_engram: None,
                captured: HashMap::new(),
            }),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Stamp, ring and send. `None` when the change was a duplicate inside
    /// [`DUPLICATE_WINDOW`] and nothing was announced. The sequence moves
    /// under the ring's mutex, so ring order and id order are one order.
    pub fn announce(&self, mut change: Change) -> Option<EventId> {
        let mut ring = self.lock();
        if let Some(audience) = ring.captured.get(change.domain()) {
            change.stamp(audience);
        }
        if let Change::Engram(engram) = &change {
            let now = Instant::now();
            if let Some((last, at)) = &ring.last_engram
                && now.duration_since(*at) < DUPLICATE_WINDOW
                && last.domain == engram.domain
                && last.path == engram.path
                && last.kind == engram.kind
                && last.checksum == engram.checksum
                && last.draft_of == engram.draft_of
            {
                return None;
            }
            ring.last_engram = Some((engram.clone(), now));
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let id = EventId {
            epoch: self.epoch,
            seq,
        };
        if ring.entries.len() == RING_CAPACITY {
            ring.entries.pop_front();
        }
        let envelope = Envelope { id, change };
        ring.entries.push_back(envelope.clone());
        // Sent under the guard, so the channel's order is the id order: two
        // announces racing on two threads never deliver 6 before 5. A send
        // never blocks, and one with no receiver is the ordinary case on a
        // daemonless engine.
        let _ = self.tx.send(envelope);
        drop(ring);
        Some(id)
    }

    /// Capture who may hear events under `domain` from now until
    /// [`ChangeBus::release`]: every later announcement under that name is
    /// stamped with `audience`, and so is every entry the ring already holds
    /// for it, so a replay after the privacy records moved never reaches an
    /// outsider (ruling K2). One pass over at most [`RING_CAPACITY`] entries.
    pub fn capture(&self, domain: &str, audience: DomainAudience) {
        let mut ring = self.lock();
        for entry in ring.entries.iter_mut() {
            if entry.change.domain() == domain {
                entry.change.stamp(&audience);
            }
        }
        ring.captured.insert(domain.to_string(), audience);
    }

    /// End a capture: events under `domain` go back to the ordinary check.
    /// Entries the capture stamped keep their audience.
    pub fn release(&self, domain: &str) {
        self.lock().captured.remove(domain);
    }

    /// The ring, whatever a panicking holder left behind: every mutation
    /// under this guard leaves the ring consistent, so a poisoned lock is
    /// taken over rather than turned into a panic in every later write verb.
    fn lock(&self) -> MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }

    /// The ring after `last`, read under the mutex into a `Vec` and released
    /// before anything is written to a socket.
    pub fn replay_after(&self, last: EventId) -> Replay {
        if last.epoch != self.epoch {
            return Replay::Reset;
        }
        // An id this process never issued (a forged or garbled header) is
        // answered like one from another process.
        if last.seq > self.seq.load(Ordering::Relaxed) {
            return Replay::Reset;
        }
        let ring = self.lock();
        if let Some(oldest) = ring.entries.front()
            && oldest.id.seq > last.seq.saturating_add(1)
        {
            return Replay::Reset;
        }
        Replay::Events(
            ring.entries
                .iter()
                .filter(|entry| entry.id.seq > last.seq)
                .cloned()
                .collect(),
        )
    }

    /// The newest id announced, or `None` before the first announcement.
    pub fn last_id(&self) -> Option<EventId> {
        let seq = self.seq.load(Ordering::Relaxed);
        (seq > 0).then_some(EventId {
            epoch: self.epoch,
            seq,
        })
    }

    pub fn ring_len(&self) -> usize {
        self.lock().entries.len()
    }
}

/// The actor label a verb that stamps no `generated` block announces: the
/// account behind an HTTP request, and nobody for the machine owner (whose
/// writes over the CLI, the control socket and a room's save carry no name a
/// reader would recognize) or the anonymous viewer.
pub fn change_label(scope: &crate::scope::Scope) -> Option<String> {
    match scope {
        crate::scope::Scope::User { account, .. } => Some(account.clone()),
        crate::scope::Scope::Unrestricted | crate::scope::Scope::Anonymous => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modified(domain: &str, path: &str, checksum: &str) -> Change {
        Change::Engram(EngramChanged {
            domain: domain.to_string(),
            permalink: path.trim_end_matches(".md").to_string(),
            path: path.to_string(),
            kind: ChangeKind::Modified,
            from: None,
            checksum: Some(checksum.to_string()),
            actor: None,
            draft_of: None,
            audience: None,
        })
    }

    #[test]
    fn ids_count_from_one_inside_the_process_epoch_and_round_trip() {
        let bus = ChangeBus::new();
        assert_eq!(bus.last_id(), None);
        let first = bus.announce(modified("eng", "a.md", "1")).unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(first.epoch, bus.epoch());
        assert_eq!(EventId::parse(&first.to_string()), Some(first));
        assert_eq!(EventId::parse("nope"), None);
        assert_eq!(EventId::parse("1:"), None);
    }

    #[test]
    fn the_ring_holds_the_capacity_and_drops_the_oldest() {
        let bus = ChangeBus::new();
        for i in 0..(RING_CAPACITY + 3) {
            bus.announce(modified("eng", &format!("{i}.md"), &i.to_string()));
        }
        assert_eq!(bus.ring_len(), RING_CAPACITY);
        let oldest_kept = EventId {
            epoch: bus.epoch(),
            seq: 3,
        };
        match bus.replay_after(oldest_kept) {
            Replay::Events(events) => {
                assert_eq!(events.len(), RING_CAPACITY);
                assert_eq!(events[0].id.seq, 4);
            }
            Replay::Reset => panic!("seq 3 is the oldest entry kept, so 4 onward replays"),
        }
        let dropped = EventId {
            epoch: bus.epoch(),
            seq: 2,
        };
        assert_eq!(
            bus.replay_after(dropped),
            Replay::Reset,
            "seq 3 was dropped"
        );
    }

    #[test]
    fn a_foreign_epoch_is_a_reset_and_the_newest_id_replays_nothing() {
        let bus = ChangeBus::new();
        let id = bus.announce(modified("eng", "a.md", "1")).unwrap();
        assert_eq!(
            bus.replay_after(EventId {
                epoch: id.epoch + 1,
                seq: 1
            }),
            Replay::Reset
        );
        assert_eq!(bus.replay_after(id), Replay::Events(Vec::new()));
    }

    #[test]
    fn a_duplicate_inside_two_seconds_is_dropped_and_a_different_one_is_not() {
        let bus = ChangeBus::new();
        assert!(bus.announce(modified("eng", "a.md", "1")).is_some());
        assert!(bus.announce(modified("eng", "a.md", "1")).is_none());
        assert!(bus.announce(modified("eng", "a.md", "2")).is_some());
        assert!(bus.announce(modified("eng", "b.md", "2")).is_some());
        assert_eq!(bus.ring_len(), 3);
    }

    #[tokio::test]
    async fn a_subscriber_hears_what_is_announced_after_it_subscribed() {
        let bus = ChangeBus::new();
        bus.announce(modified("eng", "before.md", "0"));
        let mut rx = bus.subscribe();
        let id = bus.announce(modified("eng", "after.md", "1")).unwrap();
        let heard = rx.recv().await.unwrap();
        assert_eq!(heard.id, id);
        assert_eq!(heard.change.name(), "engram");
        assert!(
            heard
                .change
                .data()
                .unwrap()
                .contains("\"path\":\"after.md\"")
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_domain_changes_audience_never_reaches_the_wire() {
        let change = Change::Domain(DomainChanged {
            domain: "eng".to_string(),
            actor: None,
            audience: Some(DomainAudience::Accounts(HashSet::from(["ada".to_string()]))),
        });
        assert_eq!(change.name(), "domain");
        assert!(matches!(
            change.audience(),
            Some(DomainAudience::Accounts(_))
        ));
        let value: serde_json::Value = serde_json::from_str(&change.data().unwrap()).unwrap();
        assert_eq!(value, serde_json::json!({ "domain": "eng", "actor": null }));
    }

    /// A forged `Last-Event-ID` at the top of the id space answers a reset
    /// and leaves the bus working: no overflow while the ring mutex is held
    /// (which in a build with overflow checks poisoned it and turned every
    /// later write verb into a panic).
    #[test]
    fn a_forged_last_id_at_u64_max_resets_and_the_bus_keeps_working() {
        let bus = ChangeBus::new();
        bus.announce(modified("eng", "a.md", "1"));
        let forged = EventId {
            epoch: bus.epoch(),
            seq: u64::MAX,
        };
        assert_eq!(bus.replay_after(forged), Replay::Reset);
        assert!(bus.announce(modified("eng", "b.md", "2")).is_some());
        assert_eq!(bus.ring_len(), 2);
        // An id ahead of anything issued is a reset too, not an empty replay
        // that would leave the client skipping what comes next.
        let ahead = EventId {
            epoch: bus.epoch(),
            seq: 3,
        };
        assert_eq!(bus.replay_after(ahead), Replay::Reset);
    }

    /// The channel delivers in id order even with announces racing on many
    /// threads: the id and the send happen under one guard.
    #[test]
    fn concurrent_announces_arrive_in_id_order() {
        let bus = std::sync::Arc::new(ChangeBus::new());
        let mut rx = bus.subscribe();
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let bus = bus.clone();
                std::thread::spawn(move || {
                    for i in 0..100 {
                        bus.announce(modified("eng", &format!("{t}-{i}.md"), "x"));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let mut last = 0;
        while let Ok(envelope) = rx.try_recv() {
            assert!(envelope.id.seq > last, "{} after {last}", envelope.id.seq);
            last = envelope.id.seq;
        }
        assert_eq!(last, 800);
    }

    /// A capture stamps what the ring already holds under that name and
    /// every later announcement under it, and nothing else; a release ends it.
    #[test]
    fn a_capture_stamps_the_ring_and_later_events_until_released() {
        let bus = ChangeBus::new();
        bus.announce(modified("eng", "a.md", "1"));
        bus.announce(modified("ops", "b.md", "1"));
        let nobody = DomainAudience::Accounts(HashSet::new());
        bus.capture("eng", nobody.clone());
        bus.announce(modified("eng", "c.md", "1"));
        let Replay::Events(events) = bus.replay_after(EventId {
            epoch: bus.epoch(),
            seq: 0,
        }) else {
            panic!("the ring holds seq 1")
        };
        let audiences: Vec<Option<&DomainAudience>> =
            events.iter().map(|e| e.change.audience()).collect();
        assert_eq!(audiences, vec![Some(&nobody), None, Some(&nobody)]);
        bus.release("eng");
        let id = bus.announce(modified("eng", "d.md", "1")).unwrap();
        let Replay::Events(events) = bus.replay_after(EventId {
            epoch: id.epoch,
            seq: id.seq - 1,
        }) else {
            panic!()
        };
        assert_eq!(events[0].change.audience(), None);
    }

    #[test]
    fn the_wire_data_carries_every_field_and_no_tag() {
        let data = modified("eng", "a.md", "9f").data().unwrap();
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["kind"], "modified");
        assert_eq!(value["from"], serde_json::Value::Null);
        assert_eq!(value["actor"], serde_json::Value::Null);
        assert!(value.get("event").is_none());
    }
}
