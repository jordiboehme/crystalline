//! Browser round trips in flight: a record per state this process generated,
//! held until the browser comes back with it or it expires.
//!
//! Three flows use it. The single sign-on relying party (the REST crate's
//! `oidc` module) keeps a sign-in's nonce, PKCE verifier and redirect uri
//! here, the GitHub App link (its `github_link` module) keeps the account and
//! the session that started a link, and the engine keeps the single-use links
//! an agent session hands its person ([`crate::engine::Engine::mint_github_link`]).
//! All need the same properties, which is why they share one store: a record
//! is taken rather than read, so a state authorizes exactly one use; it lives
//! [`PENDING_TTL`]; and the map is bounded in a way a flood cannot turn into
//! evicting a real person mid-consent.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// How long a started sign-in may take to come back. Long enough for a person
/// to read a consent screen and type a second factor, short enough that an
/// abandoned attempt is forgotten while its tab is still open.
pub const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// How many sign-ins may be in flight at once.
///
/// The OIDC login is public and unauthenticated, so the pending map is
/// something a stranger can add to. The cap is what stops that from being a
/// way to make this process reserve memory. A record is a few hundred bytes,
/// so a full map is single-digit megabytes; the number is generous next to
/// any real instance's concurrent logins and small next to what a flood would
/// want.
pub const MAX_PENDING: usize = 10_000;

/// How old a record must be before a full map may drop it to make room.
///
/// Without this, filling the map is a way to evict every real sign-in mid
/// consent: the cap would be reached and the oldest record - somebody who
/// pressed the button a few seconds ago - would go. With it, a full map of
/// records younger than this refuses the newcomer instead, which costs a
/// flood its own next request and costs the person mid-consent nothing.
pub const MIN_EVICT_AGE: Duration = Duration::from_secs(30);

/// How much slack the chronological order may carry over the records it
/// indexes before a whole-order sweep is worth its cost. Small enough that the
/// order cannot grow by much on a quiet instance, large enough that a store
/// holding a handful of sign-ins never sweeps at all. See
/// `PendingStore::compact_if_slack`.
pub const COMPACT_SLACK: usize = 16;

/// A record the store can age: when the flow it belongs to started.
pub trait PendingRecord {
    /// When the record was made, for the TTL and for the eviction order.
    fn started(&self) -> Instant;
}

/// The round trips in flight, keyed by state.
pub struct PendingStore<T> {
    records: HashMap<String, T>,
    /// Every state inserted, oldest first. Records are only ever inserted
    /// with a fresh `started`, so this is chronological, and expiry and
    /// eviction both walk it from the front in O(1) per record. An entry
    /// whose record was already taken is skipped when it is reached.
    order: VecDeque<(Instant, String)>,
}

impl<T> Default for PendingStore<T> {
    fn default() -> PendingStore<T> {
        PendingStore {
            records: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

/// The map is full of sign-ins too young to evict. See [`MIN_EVICT_AGE`].
#[derive(Debug)]
pub struct PendingFull;

/// Why a state had no record to take: it names none, or it named one that
/// outlived [`PENDING_TTL`]. Both answer the same sentence; they warn
/// differently, because an operator reading "expired" checks the provider's
/// consent screen and one reading "unknown" checks for a replay.
#[derive(Debug, PartialEq, Eq)]
pub enum StateMiss {
    Unknown,
    Expired,
}

impl<T: PendingRecord> PendingStore<T> {
    /// How many sign-ins are in flight.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// How many entries the chronological order holds, live or spent.
    #[cfg(test)]
    pub(crate) fn order_len(&self) -> usize {
        self.order.len()
    }

    /// [`PendingStore::insert_at`] on the system clock.
    pub fn insert(&mut self, state: String, pending: T) -> Result<(), PendingFull> {
        self.insert_at(state, pending, Instant::now())
    }

    /// Remember `pending` under `state`, forgetting whatever has expired by
    /// `now` and, if the map is still full, the oldest records past
    /// [`MIN_EVICT_AGE`]. Refuses when the map is full of records younger
    /// than that: a flood must not be able to push a real sign-in out from
    /// under someone.
    ///
    /// `now` and the record's `started` come from the same clock, which is
    /// the system clock for a sign-in and the engine's link clock for a
    /// GitHub link (a test steps that one past the TTL).
    pub fn insert_at(
        &mut self,
        state: String,
        pending: T,
        now: Instant,
    ) -> Result<(), PendingFull> {
        self.purge_expired(now);
        self.compact_if_slack(now);
        while self.records.len() >= MAX_PENDING {
            let Some((started, key)) = self.order.front() else {
                break;
            };
            let already_taken = !self.records.contains_key(key);
            if !already_taken && now.duration_since(*started) < MIN_EVICT_AGE {
                return Err(PendingFull);
            }
            let (_, key) = self.order.pop_front().expect("checked just above");
            self.records.remove(&key);
        }
        self.order.push_back((pending.started(), state.clone()));
        self.records.insert(state, pending);
        Ok(())
    }

    /// [`PendingStore::take_at`] on the system clock.
    pub fn take(&mut self, state: &str) -> Result<T, StateMiss> {
        self.take_at(state, Instant::now())
    }

    /// Take the record for `state`, if it exists and has not expired by
    /// `now`.
    ///
    /// Taking rather than reading is the whole point: an authorization code
    /// may be presented once, so the record that authorizes presenting it is
    /// removed before the exchange runs and a replay finds nothing. Expired
    /// records are purged here too, so an idle instance does not hold them
    /// until the next login happens to insert.
    pub fn take_at(&mut self, state: &str, now: Instant) -> Result<T, StateMiss> {
        // Looked up before the purge, so an expired record is reported as
        // expired rather than as never having existed.
        let taken = self.records.remove(state);
        self.purge_expired(now);
        match taken {
            Some(pending) if now.duration_since(pending.started()) < PENDING_TTL => Ok(pending),
            Some(_) => Err(StateMiss::Expired),
            None => Err(StateMiss::Unknown),
        }
    }

    /// Forget every record older than [`PENDING_TTL`], from the front of the
    /// chronological order until the first one that is still live, and drop
    /// the spent entries met on the way.
    ///
    /// The spent ones matter for the bound: a sign-in started and taken at
    /// once (a login, then a callback whose exchange fails) never fills the
    /// map, so the eviction loop never runs, and without this the order would
    /// keep its entry for the full TTL at the cost of two cheap requests.
    /// Completions are roughly first in, first out, so popping spent entries
    /// from the front keeps the order close to the map.
    fn purge_expired(&mut self, now: Instant) {
        while let Some((started, key)) = self.order.front() {
            let spent = !self.records.contains_key(key);
            if !spent && now.duration_since(*started) < PENDING_TTL {
                break;
            }
            let (_, key) = self.order.pop_front().expect("checked just above");
            self.records.remove(&key);
        }
    }

    /// Sweep the whole order when it has grown far past the map it indexes.
    ///
    /// [`PendingStore::purge_expired`] walks from the front and stops at the
    /// first live record, which is what makes it O(1) per record - and what
    /// leaves one abandoned consent tab pinning everything behind it: a browser
    /// that opened a sign-in and never came back holds the front of the order
    /// for the whole TTL, and every spent entry behind it waits there with it.
    /// Nothing is leaked (the TTL still ends it) and no sign-in is displaced
    /// (eviction reads the same entries), but the order can hold several times
    /// the sign-ins actually in flight for ten minutes at a time.
    ///
    /// So the front walk keeps its cheap common case and this pays for the
    /// rare one, amortized: only when the order carries more than twice the
    /// live records plus a slack that keeps a nearly-empty store from ever
    /// sweeping, and only on insert, which is the path that grows it.
    fn compact_if_slack(&mut self, now: Instant) {
        if self.order.len() <= self.records.len() * 2 + COMPACT_SLACK {
            return;
        }
        let PendingStore { records, order } = self;
        order.retain(|(started, key)| {
            if now.duration_since(*started) >= PENDING_TTL {
                records.remove(key);
                return false;
            }
            records.contains_key(key)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest record: only its age.
    struct Rec {
        started: Instant,
    }

    impl PendingRecord for Rec {
        fn started(&self) -> Instant {
            self.started
        }
    }

    fn record(age: Duration) -> Rec {
        Rec {
            started: Instant::now() - age,
        }
    }

    /// Why a take missed.
    fn miss(taken: Result<Rec, StateMiss>) -> StateMiss {
        match taken {
            Ok(_) => panic!("expected the take to miss"),
            Err(miss) => miss,
        }
    }

    /// The pending map is something an unauthenticated caller can add to, so
    /// its bounds are the ones worth pinning: a record is single use, the map
    /// has a ceiling, and the ceiling cannot be used to push a real sign-in
    /// out from under someone mid-consent.
    #[test]
    fn a_flood_of_sign_ins_cannot_evict_one_that_started_moments_ago() {
        let mut store = PendingStore::default();
        // Single use.
        store
            .insert("first".to_string(), record(Duration::ZERO))
            .unwrap();
        assert!(store.take("first").is_ok());
        assert_eq!(
            miss(store.take("first")),
            StateMiss::Unknown,
            "a state may authorize exactly one code exchange"
        );

        // A real person started a sign-in a few seconds ago...
        store
            .insert("real".to_string(), record(Duration::from_secs(5)))
            .unwrap();
        // ...and then a stranger fills the map to the brim, all just now.
        let mut refused = 0;
        for i in 0..(MAX_PENDING * 2) {
            if store
                .insert(format!("flood-{i}"), record(Duration::ZERO))
                .is_err()
            {
                refused += 1;
            }
        }
        assert!(
            store.len() <= MAX_PENDING,
            "the map grew past its cap: {}",
            store.len()
        );
        assert!(
            refused > 0,
            "past the cap, a flood is refused rather than evicting"
        );
        assert!(
            store.take("real").is_ok(),
            "the sign-in that started five seconds ago survived the flood"
        );

        // A record older than the eviction age is fair game once the map is
        // full: it makes room instead of refusing.
        let mut store = PendingStore::default();
        store
            .insert(
                "stale".to_string(),
                record(MIN_EVICT_AGE + Duration::from_secs(1)),
            )
            .unwrap();
        for i in 0..MAX_PENDING {
            store
                .insert(format!("fill-{i}"), record(Duration::ZERO))
                .unwrap_or_else(|_| panic!("insert {i} should have evicted the stale record"));
        }
        assert_eq!(
            miss(store.take("stale")),
            StateMiss::Unknown,
            "the stale record was evicted"
        );
        assert!(store.len() <= MAX_PENDING);

        // An expired record is forgotten by a take as well as by an insert,
        // so an idle instance does not hold expired sign-ins until the next
        // login happens to purge them.
        let mut store = PendingStore::default();
        store
            .insert(
                "expired".to_string(),
                record(PENDING_TTL + Duration::from_secs(1)),
            )
            .unwrap();
        assert_eq!(miss(store.take("nobody")), StateMiss::Unknown);
        assert_eq!(store.len(), 0, "a take purges what has expired");
        // And one that is asked for by name after it expired says so.
        store
            .insert(
                "late".to_string(),
                record(PENDING_TTL + Duration::from_secs(1)),
            )
            .unwrap();
        assert_eq!(miss(store.take("late")), StateMiss::Expired);
    }

    /// The chronological order is bounded by the cap too, not only the map:
    /// a caller who starts a sign-in and spends it at once (a login, then a
    /// callback whose exchange fails) leaves the map empty and would otherwise
    /// leave an entry in the order for the full ten minutes, growing it by
    /// two cheap unauthenticated requests per entry.
    #[test]
    fn spending_sign_ins_does_not_grow_the_order_past_the_cap() {
        let mut store = PendingStore::default();
        for i in 0..(MAX_PENDING * 2) {
            let state = format!("cycle-{i}");
            store.insert(state.clone(), record(Duration::ZERO)).unwrap();
            assert!(store.take(&state).is_ok());
        }
        assert_eq!(store.len(), 0);
        assert!(
            store.order_len() <= MAX_PENDING,
            "spent entries piled up in the order: {}",
            store.order_len()
        );
    }

    /// One abandoned consent tab must not pin the order behind it.
    ///
    /// The front walk stops at the first live record, so a sign-in that was
    /// started and never finished holds the front for the whole TTL while every
    /// spent entry behind it waits with it. The whole-order sweep is what keeps
    /// that bounded, and it is what this test would notice the absence of: the
    /// order would otherwise grow one entry per cycle for ten minutes.
    #[test]
    fn an_abandoned_sign_in_does_not_pin_the_spent_entries_behind_it() {
        let mut store = PendingStore::default();
        // The tab somebody opened and walked away from, at the front and live.
        store
            .insert("abandoned".to_string(), record(Duration::ZERO))
            .unwrap();
        for i in 0..200 {
            let state = format!("cycle-{i}");
            store.insert(state.clone(), record(Duration::ZERO)).unwrap();
            assert!(store.take(&state).is_ok());
        }
        assert_eq!(store.len(), 1, "the abandoned sign-in is still in flight");
        assert!(
            store.order_len() <= COMPACT_SLACK + 2,
            "spent entries piled up behind the abandoned one: {}",
            store.order_len()
        );
        // And it is still takeable: compaction forgets nothing that is live.
        assert!(store.take("abandoned").is_ok());
    }

    /// The explicit clock: a record is live just inside the TTL and expired
    /// just past it, measured against the `now` given rather than the system
    /// clock.
    #[test]
    fn the_ttl_is_measured_against_the_clock_given() {
        let base = Instant::now();
        let mut store = PendingStore::default();
        store
            .insert_at("in".to_string(), Rec { started: base }, base)
            .unwrap();
        assert!(
            store
                .take_at("in", base + PENDING_TTL - Duration::from_secs(1))
                .is_ok()
        );
        let mut store2 = PendingStore::default();
        store2
            .insert_at("out".to_string(), Rec { started: base }, base)
            .unwrap();
        assert_eq!(
            miss(store2.take_at("out", base + PENDING_TTL + Duration::from_secs(1))),
            StateMiss::Expired
        );
    }
}
