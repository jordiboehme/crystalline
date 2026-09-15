//! Working inside somebody else's draft: who is doing it, and for how long.
//!
//! A share-link grants **visibility** of one draft. Editing it is a second,
//! explicit step, and the two are deliberately not the same state: a person
//! who opens a link to read what a colleague drafted has not agreed to type
//! into their work, and an agent that holds its account's grants has not been
//! told to edit anything. So a grant lets a write be routed and a **join** is
//! what routes it.
//!
//! **A join belongs to a session, never to an account.** That is the whole
//! reason this registry exists rather than a column beside the grant. An
//! account is one person or one agent across every window and every
//! connection they have open; a session is one of those. A join held by the
//! account would mean a person who joined a draft in one browser tab had
//! silently joined it in every other tab, and - worse - that their agent,
//! which authenticates as the same account, was now writing into somebody
//! else's draft because its person pressed a button in a browser. A user's
//! join never lets an agent write into the owner's draft, and an agent's join
//! never moves the user's browser, and keying on the session is what makes
//! both true.
//!
//! **Two kinds of session hold a join, and they reach this registry the same
//! way.** The browser session is the first: Fluid opens a join, keeps the key
//! in session storage - never local storage, so the join dies with the window
//! rather than outliving it on disk - and sends it back with every write. The
//! MCP session is the second: an agent joins through the existing verbs and
//! its join is keyed by the session it joined in, so the join ends when that
//! session does. This module is written for both; only the browser half is
//! wired today.
//!
//! Nothing here is persisted. A join is the shortest-lived thing in the
//! system - it lasts as long as one session, and a daemon restart ends every
//! session there is - so a table of joins would only ever be a table of joins
//! that are already over.

use std::collections::HashMap;
use std::sync::Mutex;

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

/// One session working inside one draft that is not its own.
///
/// The four fields are the whole of it, and each is load bearing: `account` is
/// who the join was opened for and is checked on every use, so a key that
/// leaked to somebody else opens nothing; `domain` and `path` say which draft,
/// because a join is to one draft rather than to a person; and `owner` is
/// whose overlay the writes land in.
#[derive(Clone, Debug, PartialEq)]
pub struct Join {
    /// The account that opened the join. Every consumption re-checks it.
    pub account: String,
    /// The domain the draft lives in.
    pub domain: String,
    /// The domain-relative path of the draft being worked in.
    pub path: String,
    /// Whose draft it is: the actor whose overlay the writes land in.
    pub owner: String,
}

/// The joins this process is holding, keyed by session.
///
/// The key is minted here rather than taken from the caller, for the reason
/// every other credential in this codebase is: a key a caller chose would be a
/// key another caller could guess. It is still not a credential on its own -
/// [`Joins::get`] refuses a key presented by an account other than the one it
/// was opened for - so a leaked key is a dead key rather than a way into
/// somebody's draft.
#[derive(Default)]
pub struct Joins {
    open: Mutex<HashMap<String, Join>>,
}

impl Joins {
    /// Open a join and answer the key the session presents it with.
    ///
    /// **Joining a draft this account is already inside answers the join it is
    /// already holding** rather than minting a second. Pressing the button
    /// twice, reloading the page the link landed on, opening it again after a
    /// week - none of those is a new piece of work, and each of them minting a
    /// fresh key would make the caps below measure repetition rather than
    /// concurrency. It also makes Leave mean what it says: one key to give
    /// back, not however many the button was pressed.
    ///
    /// [`Err`] when a cap is met, which the route reports as a refusal rather
    /// than silently working without a join: a write that thought it had joined
    /// and had not would land in the wrong overlay, which is the one outcome
    /// this whole mechanism exists to decide deliberately.
    pub fn open(&self, join: Join) -> Result<String, JoinRefusal> {
        let mut open = self.lock();
        let mut held = 0usize;
        for (key, existing) in open.iter() {
            if existing.account != join.account {
                continue;
            }
            held += 1;
            if existing.domain == join.domain && existing.path == join.path {
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
        open.insert(key.clone(), join);
        Ok(key)
    }

    /// The join this key names, when `account` is the account it was opened
    /// for. `None` for an unknown key and for a key belonging to somebody
    /// else, deliberately one answer for both.
    pub fn get(&self, key: &str, account: &str) -> Option<Join> {
        self.lock()
            .get(key)
            .filter(|join| join.account == account)
            .cloned()
    }

    /// End one join. `false` when the key names none of `account`'s, which is
    /// what an already-closed join and somebody else's key both answer.
    pub fn close(&self, key: &str, account: &str) -> bool {
        let mut open = self.lock();
        match open.get(key) {
            Some(join) if join.account == account => {
                open.remove(key);
                true
            }
            _ => false,
        }
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
        open.retain(|_, join| !(join.domain == domain && join.owner == owner && join.path == path));
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
        open.retain(|_, join| join.domain != domain);
        before - open.len()
    }

    /// The map, recovering from a panic that poisoned it.
    ///
    /// A poisoned lock here is a panic somewhere else in this process while
    /// the map was held; the map itself is a plain `HashMap` and cannot be
    /// left half-written by one, so the contents are as sound afterwards as
    /// before. Refusing every join from then on would turn an unrelated panic
    /// into an outage of this surface.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Join>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(account: &str) -> Join {
        Join {
            account: account.to_string(),
            domain: "team".to_string(),
            path: "plan.md".to_string(),
            owner: "alice".to_string(),
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
        assert!(!joins.close(&key, "carol"), "and cannot end it either");
        assert!(joins.close(&key, "bob"));
        assert_eq!(joins.get(&key, "bob"), None, "a closed join is gone");
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
        assert!(joins.close(&first, "bob"));
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

    /// One account cannot take the instance's joins away from everybody else.
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
        assert!(
            joins.open(join("carol")).is_ok(),
            "while everybody else is unaffected, which is the point of the share"
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
