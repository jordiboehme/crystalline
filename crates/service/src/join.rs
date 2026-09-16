//! Working inside somebody else's draft: who is doing it, and for how long.
//!
//! A share-link grants **visibility** of one draft. Editing it is a second,
//! explicit step, and the two are deliberately not the same state: a person
//! who opens a link to read what a colleague drafted has not agreed to type
//! into their work, and an agent that holds its account's grants has not been
//! told to edit anything. So a grant lets a write be routed and a **join** is
//! what routes it.
//!
//! **A join belongs to a HOLDER, never to an account.** That is the whole
//! reason this registry exists rather than a column beside the grant. An
//! account is one person or one agent across every window and every
//! connection they have open; a holder is one of those. A join held by the
//! account would mean a person who joined a draft in one browser tab had
//! silently joined it in every other tab, and - worse - that their agent,
//! which authenticates as the same account, was now writing into somebody
//! else's draft because its person pressed a button in a browser. A person's
//! join never lets an agent write into the owner's draft, an agent's join
//! never opens a room in the person's browser, and [`Holder`] is what makes
//! both true.
//!
//! **[`Holder`] is the thing whose ending ends the join**, and there are four
//! kinds because there are four ways of being one caller over time:
//!
//! - a **browser session**, which ends when the person signs out or the window
//!   goes (Fluid keeps its join key in session storage, never local, for
//!   exactly that reason);
//! - an **MCP session**, the legacy `Mcp-Session-Id` lifecycle, which ends when
//!   the transport ends it;
//! - a **process**, which is a stdio agent or one connection on the daemon's
//!   own socket, and ends when that process or connection does;
//! - a **token identity**, which is what a modern-era peer on streamable HTTP
//!   has instead of a session: it is routed statelessly, so every request of
//!   its is a fresh server object and nothing on that path can be the end of
//!   anything. It is the one kind [`Joins::end_holder`] must never be asked to
//!   end for somebody, because there is no ending to report.
//!
//! **Every join ends by IDLENESS too** ([`IDLE_JOIN_LIMIT`], refreshed by
//! every use), and the holder's own ending is the FAST PATH rather than the
//! only one. Three of the four kinds do end, and when they do the sweep is
//! immediate - but nothing in this process can assert that the ending was
//! reported: a legacy session whose transport never called back, a browser
//! whose sign-out never reached us, a connection dropped mid-flight would each
//! leave a join standing for as long as the daemon does. With the window over
//! all four, the worst of that is half an hour of an author's draft holding
//! somebody who is not there, and the one holder that has no ending at all is
//! no longer a special case but the case the rule was written for.
//!
//! **A join opened on a share-link also ends when the link's own window
//! does** ([`Join::expires_at`]). The window is carried in at open time rather
//! than re-read, so nothing on the saver's four-times-a-second path asks the
//! database a question the grant answered once.
//!
//! Every question this registry answers is asked with a holder, so two holders
//! of one account are two callers here however identical their credentials
//! are. **The per-account cap is deliberately still per ACCOUNT**
//! ([`MAX_JOINS_PER_ACCOUNT`], counted across that account's holders): the cap
//! is about how much of this process one person may take, and opening a second
//! window is not a reason to be allowed twice as much.
//!
//! **A read that presents a link opens the draft too**, which the separation
//! above does not obviously allow. It is deliberate and it is the agent's case
//! rather than the person's: a browser has two buttons and a person presses
//! them a minute apart, while an agent that was handed a link and passed it to
//! a verb has decided both at once and has no second surface to decide the
//! rest on. The grant is still what a read crosses on, and the join is still
//! what routes a write - a caller who may only READ the draft redeems the link
//! and reads it, and is refused the join in the words that say why.
//!
//! Nothing here is persisted. A join is the shortest-lived thing in the
//! system - it lasts as long as one holder, and a daemon restart ends every
//! holder there is - so a table of joins would only ever be a table of joins
//! that are already over.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::{OsRng, RngCore};

/// How many joins this process holds open at once, across everybody.
///
/// A cap rather than trust: opening a join is a route any signed-in account
/// may drive, and a registry with no ceiling is a map a caller can grow
/// without limit. The number is far above what any real instance reaches - a
/// join is one person working in one draft - so the only caller that meets it
/// is one that was filling the map on purpose.
pub const MAX_OPEN_JOINS: usize = 1024;

/// How many joins one account may hold open at once.
///
/// The global cap alone is not a bound on anybody: one signed-in account
/// holding one live link could open a join a thousand times over and every
/// other person on the instance would then be refused one until the daemon
/// restarted. This is the share, and it is generous for the thing it measures -
/// a join is one person working in one draft, and the dedup below means
/// rejoining the same draft costs nothing at all.
pub const MAX_JOINS_PER_ACCOUNT: usize = 16;

/// Why a join was not opened, in words the route can hand to a person.
///
/// Two outcomes rather than an `Option`, because they are two different facts
/// and only one of them is about the caller: one instance is full and one
/// person is holding too many.
#[derive(Debug, PartialEq)]
pub enum JoinRefusal {
    /// This process is already holding [`MAX_OPEN_JOINS`].
    InstanceFull,
    /// This account is already holding [`MAX_JOINS_PER_ACCOUNT`].
    AccountFull,
}

/// How long ANY join survives with nothing using it.
///
/// It is refreshed by every use - `get`, `holds` and `held_by` all touch - so
/// a caller that goes on working in a draft stays in it and one that walked
/// away half an hour ago does not. An open room refreshes on its own: the
/// saver asks [`Joins::holds`] about every guest four times a second, so a
/// person reading somebody's draft without typing is using their join the
/// whole time they have it open.
///
/// **Over every holder, not only [`Holder::Token`].** That holder has no
/// ending of its own and this is the whole of what ends it; the other three do
/// end, and their ending is the fast path. What the window adds for them is
/// the guarantee that a reported ending is not the ONLY way out - an ending
/// that never arrives leaves a join standing for half an hour rather than for
/// as long as this process runs.
pub const IDLE_JOIN_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Who is holding a join: the thing whose ending ends it.
///
/// Never an account. See the module doc for why the distinction is the whole
/// point of this registry, and for what each kind's ending is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    /// One browser session, named by the CSRF token of the session it came
    /// from - the one per-session value every identity path already resolves,
    /// and the only one a WebSocket upgrade can be asked about (a browser can
    /// put no header on an upgrade, and a key in a query string is written to
    /// every log and proxy on the way).
    Browser(String),
    /// One legacy MCP session, named by its `Mcp-Session-Id`.
    McpSession(String),
    /// One process or socket connection serving one agent: a stdio server, or
    /// one connection on the daemon's own `mcp` socket. Numbered per server
    /// object, because a daemon serves many of them at once.
    Process(u64),
    /// A modern-era peer with no session at all, named by the account its
    /// token resolved to. Ended by idleness ([`IDLE_JOIN_LIMIT`]) and by
    /// nothing else, because it has no ending of its own to be swept on: see
    /// [`Holder::ends_with_its_holder`].
    Token(String),
}

impl Holder {
    /// Whether [`Joins::end_holder`] may sweep for this holder: whether the
    /// thing holding the join is an object whose ending somebody can report.
    ///
    /// **A fast path rather than the only path.** Every join is idle-limited
    /// ([`IDLE_JOIN_LIMIT`]), so this decides how QUICKLY a join ends, not
    /// whether it does. True for every holder that is an object with a
    /// lifetime somebody can observe, and the sweep on its ending is what
    /// makes a signed-out window leave the draft at once instead of half an
    /// hour later.
    ///
    /// False for [`Holder::Token`], which is an identity rather than an
    /// object: on the stateless path a fresh server is built per request, so
    /// ending its joins when it goes would end them after one call - and, were
    /// the dedup ever account-shaped again, would end somebody's browser join
    /// from inside their agent's request.
    pub fn ends_with_its_holder(&self) -> bool {
        !matches!(self, Holder::Token(_))
    }
}

/// One holder working inside one draft that is not its own.
///
/// The six fields are the whole of it, and each is load bearing: `account` is
/// who the join was opened for and is checked on every use, so a key that
/// leaked to somebody else opens nothing; `holder` is WHICH of that account's
/// callers is inside the draft, so one window's join is not another's and an
/// agent's is neither; `domain` and `path` say which draft, because a join is
/// to one draft rather than to a person; and `owner` is whose overlay the
/// writes land in.
#[derive(Clone, Debug, PartialEq)]
pub struct Join {
    /// The account that opened the join. Every consumption re-checks it.
    pub account: String,
    /// Which of that account's callers opened it, and whose ending ends it.
    pub holder: Holder,
    /// The domain the draft lives in.
    pub domain: String,
    /// The domain-relative path of the draft being worked in.
    pub path: String,
    /// Whose draft it is: the actor whose overlay the writes land in.
    pub owner: String,
    /// When the share-link this join was opened on runs out, on this
    /// process's clock, or `None` for a link with no window at all.
    ///
    /// **The grant's window, carried rather than re-read.** A revoked link
    /// ends its joins where the revocation happens, and a folded, discarded or
    /// renamed draft ends them where that happens - an expiry is the one
    /// ending nothing announces, because it is a moment passing rather than
    /// somebody acting. So the moment travels with the join and
    /// [`Joins::expire_idle`] reads it, which keeps the saver's pass asking
    /// this process's memory rather than the database four times a second.
    ///
    /// Stamped once, at [`Joins::open`], and sound because a grant is
    /// immutable apart from being revoked: its window is set when the author
    /// mints the link and is never moved afterwards. See [`grant_deadline`].
    pub expires_at: Option<Instant>,
}

/// When a join opened on a share-link stops being one, on this process's
/// clock, or `None` for a link that lasts as long as the draft does.
///
/// The grant's `expires_at` is RFC 3339 wall-clock and this registry measures
/// in [`Instant`]s, so the window is converted once, here, where the link is
/// presented. An instant that cannot be parsed reads as already over, which is
/// the rule the store itself redeems by - a grant nobody can date is one
/// nobody may work inside - and is unreachable from a redeemed grant, since
/// that check has already refused it.
pub fn grant_deadline(expires_at: Option<&str>) -> Option<Instant> {
    let at = expires_at?;
    let now = Instant::now();
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(at) else {
        return Some(now);
    };
    let left = at.signed_duration_since(chrono::Utc::now());
    Some(now + left.to_std().unwrap_or(Duration::ZERO))
}

/// The later of two link windows, where `None` is a link with no window.
///
/// What a holder rejoining one draft on a second link stands on: they hold
/// both links, so their access ends when the last of the two does. Taking the
/// newer window instead would let presenting a short-lived link shorten a
/// standing join the holder already had another reason to keep.
///
/// A link with no window keeping the join unbounded is not a way to outlive a
/// revocation: taking a link back ends every join on that draft outright
/// ([`Joins::end_draft`]), so the only thing a window has to bound is the case
/// where nothing else ends it at all.
fn later(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (None, _) | (_, None) => None,
        (Some(a), Some(b)) => Some(a.max(b)),
    }
}

/// The joins this process is holding, keyed by the key each holder presents.
///
/// The key is minted here rather than taken from the caller, for the reason
/// every other credential in this codebase is: a key a caller chose would be a
/// key another caller could guess. It is still not a credential on its own -
/// [`Joins::get`] refuses a key presented by an account other than the one it
/// was opened for - so a leaked key is a dead key rather than a way into
/// somebody's draft.
#[derive(Default)]
pub struct Joins {
    open: Mutex<HashMap<String, Held>>,
}

/// One open join and when it was last used, which is what
/// [`Joins::expire_idle`] reads.
struct Held {
    join: Join,
    last_used: Instant,
}

impl Joins {
    /// Open a join and answer the key the holder presents it with.
    ///
    /// **Joining a draft this HOLDER is already inside answers the key it is
    /// already holding** rather than minting a second. Pressing the button
    /// twice, reloading the page the link landed on, opening it again after a
    /// week - none of those is a new piece of work, and each of them minting a
    /// fresh key would make the caps below measure repetition rather than
    /// concurrency. It also makes Leave mean what it says: one key to give
    /// back, not however many the button was pressed.
    ///
    /// **The dedup is per holder and not per account**, which is the rule the
    /// module doc exists for: another window of the same person, and that
    /// person's agent, are other holders and get keys of their own. Sharing one
    /// key between them would mean whichever of them ended first put the others
    /// out of the draft.
    ///
    /// [`Err`] when a cap is met, which the route reports as a refusal rather
    /// than silently working without a join: a write that thought it had joined
    /// and had not would land in the wrong overlay, which is the one outcome
    /// this whole mechanism exists to decide deliberately.
    pub fn open(&self, join: Join) -> Result<String, JoinRefusal> {
        let now = Instant::now();
        let mut open = self.lock();
        Self::expire_locked(&mut open, now);
        let mut held = 0usize;
        for (key, existing) in open.iter_mut() {
            if existing.join.account != join.account {
                continue;
            }
            // Counted per ACCOUNT across every holder it has, deliberately:
            // the cap is about how much of this process one person may take,
            // and opening a second window is not a reason to be allowed twice
            // as much.
            held += 1;
            // All four, and `owner` is the one that is easy to forget: two
            // authors routinely hold a draft at the same path, because an
            // overlay row is keyed by actor as well as by path. Dedupping on
            // the path alone would hand somebody joining a SECOND author's
            // draft the key they hold for the first, and every write made
            // "inside" the second would be routed into the first author's
            // overlay. [`Joins::end_draft`] names a draft the same three ways,
            // and the two have to agree about what one draft is.
            if existing.join.holder == join.holder
                && existing.join.domain == join.domain
                && existing.join.owner == join.owner
                && existing.join.path == join.path
            {
                existing.last_used = now;
                existing.join.expires_at = later(existing.join.expires_at, join.expires_at);
                return Ok(key.clone());
            }
        }
        if held >= MAX_JOINS_PER_ACCOUNT {
            return Err(JoinRefusal::AccountFull);
        }
        if open.len() >= MAX_OPEN_JOINS {
            return Err(JoinRefusal::InstanceFull);
        }
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let key = crystalline_index::hex_lower(&bytes);
        open.insert(
            key.clone(),
            Held {
                join,
                last_used: now,
            },
        );
        Ok(key)
    }

    /// The join this key names, when `account` is the account it was opened
    /// for. `None` for an unknown key and for a key belonging to somebody
    /// else, deliberately one answer for both. A hit is a use, so it refreshes
    /// an idle-limited holder's clock.
    pub fn get(&self, key: &str, account: &str) -> Option<Join> {
        let now = Instant::now();
        let mut open = self.lock();
        Self::expire_locked(&mut open, now);
        let held = open.get_mut(key).filter(|h| h.join.account == account)?;
        held.last_used = now;
        Some(held.join.clone())
    }

    /// Whether this HOLDER is inside exactly this draft right now.
    ///
    /// The question a surface asks when it has no key to present: a browser
    /// cannot put a header on a WebSocket upgrade, and a key in a URL would be
    /// written to every log and proxy between here and the page. So the collab
    /// upgrade asks whether the browser session this request came from holds a
    /// join into the draft it is being asked to open, rather than asking it to
    /// prove one.
    ///
    /// **The account rides along as defence rather than as the question.** A
    /// holder is unique to one principal on every path that mints one, so the
    /// account clause can never be what decides this - but the account binding
    /// of a routed write used to rest on [`Joins::get`], which checks it, and
    /// asking the holder alone would have left that binding resting on holder
    /// uniqueness and nothing else. One comparison keeps it stated where it is
    /// relied on.
    ///
    /// It is a live comparison on every path, including stdio: a stdio caller's
    /// scope is `Scope::Unrestricted`, whose `overlay_actor` is the owner
    /// identity rather than nothing, so that caller opens joins under an
    /// account like any other and is compared here like any other.
    ///
    /// **It is the holder that is asked, never the account.** An agent that
    /// joined a draft has not opened a room in its person's browser, and a
    /// window that joined one has not opened a room in the window beside it;
    /// each of those is a different holder, and each has to join for itself.
    /// That is the same triple [`Joins::open`] dedups on, so one holder is
    /// inside one draft once, whatever key it holds.
    pub fn holds(
        &self,
        account: &str,
        holder: &Holder,
        domain: &str,
        owner: &str,
        path: &str,
    ) -> bool {
        let now = Instant::now();
        let mut open = self.lock();
        Self::expire_locked(&mut open, now);
        let mut found = false;
        for held in open.values_mut() {
            if held.join.account == account
                && &held.join.holder == holder
                && held.join.domain == domain
                && held.join.owner == owner
                && held.join.path == path
            {
                held.last_used = now;
                found = true;
            }
        }
        found
    }

    /// Every join this HOLDER is inside in one domain, of `account`'s.
    ///
    /// The account is the same defence it is in [`Joins::holds`]: the holder
    /// decides, and the account is what a routed write's binding used to rest
    /// on and now goes on resting on here too.
    ///
    /// The registry rather than a list the caller kept, and that is the whole
    /// of the stateless story: a modern-era peer is a fresh server object per
    /// request, so anything it remembered between two calls it would have
    /// forgotten. Asking here means its second request finds the join its
    /// first one opened, and finds it ended the moment a revoke, a rename, a
    /// discard or a fold ended it.
    pub fn held_by(&self, account: &str, holder: &Holder, domain: &str) -> Vec<Join> {
        let now = Instant::now();
        let mut open = self.lock();
        Self::expire_locked(&mut open, now);
        let mut found = Vec::new();
        for held in open.values_mut() {
            if held.join.account == account
                && &held.join.holder == holder
                && held.join.domain == domain
            {
                held.last_used = now;
                found.push(held.join.clone());
            }
        }
        found
    }

    /// End one join. `false` when the key names none of `account`'s
    /// `holder`'s, which is what an already-closed join, somebody else's key
    /// and another holder of the same account all answer.
    ///
    /// **Guarded on the holder too, not the account alone.** One account can
    /// hold several joins at once through several holders - another window
    /// of the same person, that person's agent - and each is its own object
    /// with its own lifetime; a close keyed on the account alone would let
    /// one holder end another's join, closing a page out from under whoever
    /// actually opened it.
    pub fn close(&self, key: &str, account: &str, holder: &Holder) -> bool {
        let mut open = self.lock();
        match open.get(key) {
            Some(held) if held.join.account == account && &held.join.holder == holder => {
                open.remove(key);
                true
            }
            _ => false,
        }
    }

    /// End every join ONE holder is inside, because that holder has ended.
    ///
    /// Answers how many were open. **The fast path out of a draft**, not the
    /// only one: every join is idle-limited as well ([`IDLE_JOIN_LIMIT`]), so
    /// a holder whose ending is never reported leaves the draft half an hour
    /// later instead of never. This is what makes the ordinary case immediate.
    ///
    /// A [`Holder::Token`] is refused rather than swept, and the refusal is
    /// the point: that holder is an identity rather than an object, so "it
    /// ended" is never a true thing for a caller to say about it - the
    /// per-request server object that would say it is not the end of
    /// anything, and sweeping on it would put an agent out of a draft after
    /// one call. Idleness is that holder's only ending; see
    /// [`Joins::expire_idle`].
    pub fn end_holder(&self, holder: &Holder) -> usize {
        if !holder.ends_with_its_holder() {
            return 0;
        }
        let mut open = self.lock();
        let before = open.len();
        open.retain(|_, held| &held.join.holder != holder);
        before - open.len()
    }

    /// End every join into ONE draft, because that draft has ended. Answers
    /// how many were open.
    ///
    /// A session left inside a draft that is gone would be a session whose
    /// every write is refused by a sentence about a page nobody holds, and
    /// whose bar goes on naming work that is not there.
    pub fn end_draft(&self, domain: &str, owner: &str, path: &str) -> usize {
        let mut open = self.lock();
        let before = open.len();
        open.retain(|_, held| {
            !(held.join.domain == domain && held.join.owner == owner && held.join.path == path)
        });
        before - open.len()
    }

    /// End every join into one domain, because every draft in it has ended.
    ///
    /// The counterpart of `AuthStore::end_domain_overlay_grants`: leaving
    /// review mode folds or discards every draft there is, so a session still
    /// joined to one would be joined to nothing. Answers how many were open.
    pub fn end_domain(&self, domain: &str) -> usize {
        let mut open = self.lock();
        let before = open.len();
        open.retain(|_, held| held.join.domain != domain);
        before - open.len()
    }

    /// End every join that has run out as of `now`: one nothing has used for
    /// [`IDLE_JOIN_LIMIT`], and one whose share-link's own window has passed.
    /// Answers how many were ended.
    ///
    /// **Whatever holds it.** A holder that can report its ending still ends
    /// its joins the moment it does ([`Joins::end_holder`]); this is what
    /// happens when no such report ever comes.
    ///
    /// Takes `now` rather than reading the clock, so a test drives the window
    /// as a value instead of waiting half an hour - the same shape the
    /// co-editing saver's `tick_save` takes. Every registry question runs it
    /// first, so there is no sweeper task and no join outlives its window by
    /// more than the gap between two uses of this registry.
    pub fn expire_idle(&self, now: Instant) -> usize {
        Self::expire_locked(&mut self.lock(), now)
    }

    /// [`Joins::expire_idle`] over the locked map.
    fn expire_locked(open: &mut HashMap<String, Held>, now: Instant) -> usize {
        let before = open.len();
        open.retain(|_, held| {
            now.saturating_duration_since(held.last_used) < IDLE_JOIN_LIMIT
                && held.join.expires_at.is_none_or(|at| now < at)
        });
        before - open.len()
    }

    /// The map, recovering from a panic that poisoned it.
    ///
    /// A poisoned lock here is a panic somewhere else in this process while
    /// the map was held; the map itself is a plain `HashMap` and cannot be
    /// left half-written by one, so the contents are as sound afterwards as
    /// before. Refusing every join from then on would turn an unrelated panic
    /// into an outage of this surface.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Held>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser(name: &str) -> Holder {
        Holder::Browser(format!("csrf-{name}"))
    }

    fn join(account: &str) -> Join {
        Join {
            account: account.to_string(),
            holder: browser(account),
            domain: "team".to_string(),
            path: "plan.md".to_string(),
            owner: "alice".to_string(),
            expires_at: None,
        }
    }

    /// The security property the whole ruling rests on: a join key is bound to
    /// the account it was opened for, so a key that leaks - pasted into a chat,
    /// copied out of a session store, guessed - opens nothing for anybody else.
    #[test]
    fn a_join_key_opens_nothing_for_another_account() {
        let joins = Joins::default();
        let key = joins.open(join("bob")).expect("a fresh registry has room");
        assert_eq!(joins.get(&key, "bob"), Some(join("bob")));
        assert_eq!(
            joins.get(&key, "carol"),
            None,
            "somebody else presenting it is told what an invented key is told"
        );
        assert!(
            !joins.close(&key, "carol", &browser("bob")),
            "and cannot end it either"
        );
        assert!(joins.close(&key, "bob", &browser("bob")));
        assert_eq!(joins.get(&key, "bob"), None, "a closed join is gone");
    }

    /// One account, two holders - another window of the same person, and
    /// that person's agent - each holding their own join over the same page.
    /// One cannot close the other's key: `close` is guarded on the holder,
    /// not the account alone, so a browser tab leaving does not close out
    /// from under the agent working the same draft, and the reverse.
    #[test]
    fn one_holder_of_an_account_cannot_close_another_holders_key() {
        let joins = Joins::default();
        let agent = Join {
            holder: Holder::Process(1),
            ..join("bob")
        };
        let browser_key = joins.open(join("bob")).unwrap();
        let agent_key = joins.open(agent).unwrap();
        assert_ne!(
            browser_key, agent_key,
            "two holders of one account over one page are two joins"
        );

        assert!(
            !joins.close(&agent_key, "bob", &browser("bob")),
            "the browser holder cannot close the agent's key"
        );
        assert_eq!(
            joins.get(&agent_key, "bob"),
            Some(Join {
                holder: Holder::Process(1),
                ..join("bob")
            }),
            "which is still open"
        );

        assert!(
            !joins.close(&browser_key, "bob", &Holder::Process(1)),
            "and the agent cannot close the browser's key either"
        );
        assert_eq!(
            joins.get(&browser_key, "bob"),
            Some(join("bob")),
            "which is still open too"
        );

        assert!(
            joins.close(&agent_key, "bob", &Holder::Process(1)),
            "each holder can still close its own"
        );
        assert!(joins.close(&browser_key, "bob", &browser("bob")));
    }

    /// Joining a draft you are already inside is not a second piece of work.
    ///
    /// Pressing the button twice, or reloading the page the link landed on,
    /// answers the join already held - so Leave has one key to give back and
    /// the caps measure how many drafts somebody is working in rather than how
    /// many times they asked.
    #[test]
    fn joining_the_same_draft_again_answers_the_join_already_held() {
        let joins = Joins::default();
        let first = joins.open(join("bob")).unwrap();
        let again = joins.open(join("bob")).unwrap();
        assert_eq!(first, again, "one join, one key");
        assert!(joins.close(&first, "bob", &browser("bob")));
        assert_eq!(joins.get(&again, "bob"), None, "and one Leave ends it");

        // Another draft is another join, which is the half this must not break.
        let other = joins
            .open(Join {
                path: "charter.md".to_string(),
                ..join("bob")
            })
            .unwrap();
        assert_ne!(other, first);
    }

    /// **Two holders of one account are two callers here.**
    ///
    /// The rule the whole module exists for, said as the three things that
    /// follow from it: each holder gets a key of its own, one holder's key
    /// opens nothing for another, and one holder ending leaves the others
    /// exactly where they were. Without it a person's agent and their browser
    /// share a key, and whichever of them ends first puts the other out of a
    /// draft they are working in.
    #[test]
    fn two_holders_of_one_account_get_two_joins() {
        let joins = Joins::default();
        let in_browser = joins.open(join("bob")).unwrap();
        let agents = joins
            .open(Join {
                holder: Holder::Token("bob".to_string()),
                ..join("bob")
            })
            .unwrap();
        assert_ne!(
            in_browser, agents,
            "one key per holder, and these are two holders"
        );
        assert!(joins.holds("bob", &browser("bob"), "team", "alice", "plan.md"));
        assert!(joins.holds(
            "bob",
            &Holder::Token("bob".to_string()),
            "team",
            "alice",
            "plan.md"
        ));

        // The agent's holder ending - which for a token identity never
        // happens - and the browser's are separate events.
        assert_eq!(joins.end_holder(&browser("bob")), 1);
        assert_eq!(
            joins.get(&in_browser, "bob"),
            None,
            "the window that signed out is out"
        );
        assert!(
            joins.get(&agents, "bob").is_some(),
            "and its agent is still working, which is the whole rule"
        );
    }

    /// A token identity has no ending to be swept on, so asking for one is
    /// answered with nothing done.
    ///
    /// The pin under the stateless transport: a modern-era peer's server
    /// object lives for one request, so if `end_holder` swept for it, an
    /// agent's join would last exactly one call - and its own `Drop` would run
    /// inside the very request that opened it.
    #[test]
    fn a_token_identitys_joins_are_never_ended_by_a_holder_ending() {
        let joins = Joins::default();
        let holder = Holder::Token("bob".to_string());
        let key = joins
            .open(Join {
                holder: holder.clone(),
                ..join("bob")
            })
            .unwrap();
        assert_eq!(joins.end_holder(&holder), 0, "there is no ending to sweep");
        assert!(joins.get(&key, "bob").is_some());
        assert!(!holder.ends_with_its_holder());
        assert!(browser("bob").ends_with_its_holder());
        assert!(Holder::McpSession("s1".to_string()).ends_with_its_holder());
        assert!(Holder::Process(7).ends_with_its_holder());
    }

    /// What ends a token identity's join instead: half an hour of nothing,
    /// and every use pushes that half hour out again.
    ///
    /// The window is the same for a holder that HAS an ending of its own - the
    /// browser join here goes with it - which is the rule stated from the
    /// other side in `a_session_holders_join_ends_when_nothing_uses_it_either`:
    /// the ending is the fast path, and idleness is the floor under all four
    /// kinds.
    #[test]
    fn a_token_identitys_join_expires_when_nothing_uses_it() {
        let joins = Joins::default();
        let holder = Holder::Token("bob".to_string());
        let key = joins
            .open(Join {
                holder: holder.clone(),
                ..join("bob")
            })
            .unwrap();
        let browsers = joins.open(join("bob")).unwrap();

        let nearly = Instant::now() + IDLE_JOIN_LIMIT - Duration::from_secs(60);
        assert_eq!(joins.expire_idle(nearly), 0, "inside the window it stands");
        // And that lookup was a use, so the window starts again from it.
        assert!(joins.get(&key, "bob").is_some());

        let past = Instant::now() + IDLE_JOIN_LIMIT + Duration::from_secs(1);
        assert_eq!(
            joins.expire_idle(past),
            2,
            "the agent's and the window nobody touched either"
        );
        assert_eq!(joins.get(&key, "bob"), None, "idle, so it is over");
        assert_eq!(joins.get(&browsers, "bob"), None, "and so is the other");
    }

    /// A join opened on a link that runs out is over when the link is, however
    /// busy the person inside it has been.
    ///
    /// The half idleness cannot cover: somebody typing in a colleague's draft
    /// refreshes their join with every keystroke, so a window that only
    /// measured quiet would keep them inside the draft long after the link
    /// they were let in on stopped opening anything.
    #[test]
    fn a_join_ends_when_the_link_it_was_opened_on_runs_out() {
        let joins = Joins::default();
        let window = Instant::now() + Duration::from_secs(10 * 60);
        let key = joins
            .open(Join {
                expires_at: Some(window),
                ..join("bob")
            })
            .unwrap();
        let forever = joins
            .open(Join {
                path: "charter.md".to_string(),
                ..join("bob")
            })
            .unwrap();

        // Busy right up to the last moment, which is what makes this a
        // different question from idleness.
        let inside = window - Duration::from_secs(1);
        assert_eq!(joins.expire_idle(inside), 0, "the link still opens it");
        assert!(joins.get(&key, "bob").is_some());

        let after = window + Duration::from_secs(1);
        assert_eq!(joins.expire_idle(after), 1);
        assert_eq!(
            joins.get(&key, "bob"),
            None,
            "the link ran out, so it is over"
        );
        assert!(
            joins.get(&forever, "bob").is_some(),
            "while a link with no window on it is untouched"
        );
    }

    /// A second link into one draft leaves the holder inside it for as long as
    /// the LONGER of the two says, because they hold both.
    #[test]
    fn rejoining_on_a_second_link_keeps_the_longer_window() {
        let joins = Joins::default();
        let soon = Instant::now() + Duration::from_secs(60);
        let later_on = Instant::now() + Duration::from_secs(20 * 60);
        let key = joins
            .open(Join {
                expires_at: Some(later_on),
                ..join("bob")
            })
            .unwrap();
        let again = joins
            .open(Join {
                expires_at: Some(soon),
                ..join("bob")
            })
            .unwrap();
        assert_eq!(key, again, "one holder inside one draft is one join");
        assert_eq!(
            joins.expire_idle(soon + Duration::from_secs(1)),
            0,
            "the shorter link running out does not put them outside the draft"
        );
        assert_eq!(joins.expire_idle(later_on + Duration::from_secs(1)), 1);
    }

    /// **Every holder's join ends when nothing uses it**, not only a token
    /// identity's.
    ///
    /// The belt to the drop's brace. A legacy MCP session's joins are swept
    /// when the transport ends the session, and nothing in this process can
    /// assert that sweep ran: if it does not, a join with no idle limit stands
    /// for as long as the daemon does, holding an author's draft open for
    /// somebody who cannot come back. With the window over every holder, the
    /// worst that happens is half an hour of it.
    #[test]
    fn a_session_holders_join_ends_when_nothing_uses_it_either() {
        let joins = Joins::default();
        let holder = Holder::McpSession("session-1".to_string());
        let key = joins
            .open(Join {
                holder: holder.clone(),
                ..join("bob")
            })
            .unwrap();

        let nearly = Instant::now() + IDLE_JOIN_LIMIT - Duration::from_secs(60);
        assert_eq!(joins.expire_idle(nearly), 0, "inside the window it stands");
        // And that lookup is a use, so the window starts again from it.
        assert!(joins.get(&key, "bob").is_some());

        let past = Instant::now() + IDLE_JOIN_LIMIT + Duration::from_secs(1);
        assert_eq!(joins.expire_idle(past), 1, "past it, the join is over");
        assert_eq!(joins.get(&key, "bob"), None);
        assert!(
            holder.ends_with_its_holder(),
            "while the session's own ending is still the fast path"
        );
    }

    /// The registry is where a stateless peer's second request finds the join
    /// its first one opened.
    #[test]
    fn held_by_answers_one_holders_joins_in_one_domain() {
        let joins = Joins::default();
        let holder = Holder::Token("bob".to_string());
        joins
            .open(Join {
                holder: holder.clone(),
                ..join("bob")
            })
            .unwrap();
        joins
            .open(Join {
                holder: holder.clone(),
                domain: "other".to_string(),
                ..join("bob")
            })
            .unwrap();
        // Somebody else's, at the same draft, must not appear.
        joins
            .open(Join {
                account: "carol".to_string(),
                holder: Holder::Token("carol".to_string()),
                ..join("carol")
            })
            .unwrap();

        let here = joins.held_by("bob", &holder, "team");
        assert_eq!(here.len(), 1, "{here:?}");
        assert_eq!(here[0].owner, "alice");
        assert_eq!(here[0].path, "plan.md");
        assert!(
            joins.held_by("bob", &browser("bob"), "team").is_empty(),
            "another holder of the same account is inside nothing"
        );
    }

    /// Two authors can hold a draft at one path, so a join is to one author's.
    ///
    /// The dedup and [`Joins::end_draft`] have to mean the same thing by "one
    /// draft": if the dedup forgot the owner, somebody joining a second
    /// author's draft at a path they already work in would be handed the first
    /// author's key, and every write they made would be routed into the wrong
    /// overlay - under the wrong name, for the wrong person to review.
    #[test]
    fn joining_a_second_authors_draft_at_the_same_path_is_a_second_join() {
        let joins = Joins::default();
        let from_alice = joins.open(join("bob")).unwrap();
        let from_carol = joins
            .open(Join {
                owner: "carol".to_string(),
                ..join("bob")
            })
            .unwrap();
        assert_ne!(
            from_alice, from_carol,
            "one key per draft, and these are two drafts"
        );
        assert_eq!(
            joins.get(&from_alice, "bob").map(|held| held.owner),
            Some("alice".to_string())
        );
        assert_eq!(
            joins.get(&from_carol, "bob").map(|held| held.owner),
            Some("carol".to_string()),
            "each key routes to the author whose draft it was opened on"
        );

        // And ending one leaves the other, which is the same agreement said
        // from the other side.
        assert_eq!(joins.end_draft("team", "alice", "plan.md"), 1);
        assert_eq!(joins.get(&from_alice, "bob"), None);
        assert!(joins.get(&from_carol, "bob").is_some());
    }

    /// One account cannot take the instance's joins away from everybody else,
    /// and the share is counted over that account's holders rather than per
    /// holder - opening a second window is not a reason to be allowed twice as
    /// much.
    #[test]
    fn one_account_is_capped_below_the_instances_own_ceiling() {
        let joins = Joins::default();
        for n in 0..MAX_JOINS_PER_ACCOUNT {
            joins
                .open(Join {
                    path: format!("page-{n}.md"),
                    ..join("bob")
                })
                .expect("up to the account's share");
        }
        assert_eq!(
            joins.open(Join {
                path: "one-too-many.md".to_string(),
                ..join("bob")
            }),
            Err(JoinRefusal::AccountFull),
            "and no further"
        );
        assert_eq!(
            joins.open(Join {
                holder: Holder::Token("bob".to_string()),
                path: "one-too-many.md".to_string(),
                ..join("bob")
            }),
            Err(JoinRefusal::AccountFull),
            "nor from another holder of the same account"
        );
        assert!(
            joins.open(join("carol")).is_ok(),
            "while everybody else is unaffected, which is the point of the share"
        );
    }

    /// The question the collab upgrade asks: is this HOLDER inside this draft?
    /// Same quadruple as the dedup and the same triple as the ending, so the
    /// three cannot mean different things by "one draft".
    #[test]
    fn holding_a_join_is_asked_by_the_holder_rather_than_by_the_key() {
        let joins = Joins::default();
        let key = joins.open(join("bob")).unwrap();
        assert!(joins.holds("bob", &browser("bob"), "team", "alice", "plan.md"));
        assert!(
            !joins.holds("carol", &browser("carol"), "team", "alice", "plan.md"),
            "somebody else is not inside it"
        );
        assert!(
            !joins.holds("bob", &browser("carol"), "team", "alice", "plan.md"),
            "and neither is somebody else's window under this account's name"
        );
        assert!(
            !joins.holds(
                "bob",
                &Holder::Token("bob".to_string()),
                "team",
                "alice",
                "plan.md"
            ),
            "and neither is another holder of the same account"
        );
        assert!(!joins.holds("bob", &browser("bob"), "team", "carol", "plan.md"));
        assert!(!joins.holds("bob", &browser("bob"), "other", "alice", "plan.md"));
        assert!(!joins.holds("bob", &browser("bob"), "team", "alice", "charter.md"));
        joins.close(&key, "bob", &browser("bob"));
        assert!(
            !joins.holds("bob", &browser("bob"), "team", "alice", "plan.md"),
            "and leaving ends it"
        );
    }

    /// Leaving review mode ends every draft in the domain at once, so it ends
    /// every join into it too - and leaves the joins into other domains alone.
    #[test]
    fn ending_a_domain_ends_its_joins_and_no_others() {
        let joins = Joins::default();
        let here = joins.open(join("bob")).unwrap();
        let elsewhere = joins
            .open(Join {
                domain: "other".to_string(),
                ..join("bob")
            })
            .unwrap();
        assert_eq!(joins.end_domain("team"), 1);
        assert_eq!(joins.get(&here, "bob"), None);
        assert!(joins.get(&elsewhere, "bob").is_some());
    }
}
