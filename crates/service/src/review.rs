//! Review mode's plan and its fold: what leaving review mode would do to every
//! actor's private drafts, and the one address rule that decides whether it can
//! be done at all.
//!
//! A domain in review mode takes every write into its author's own draft, and
//! the folder the team shares changes only through a reviewed proposal. Coming
//! back out of that ends every draft one way or the other, so it is a question
//! before it is a change: [`plan_json`] answers what would happen and writes
//! nothing, and [`choices`] insists that the answer name every actor the plan
//! named and nobody else.
//!
//! Everything here is a pure function over rows somebody else read. The verb
//! itself - the gates, the config key, the file writes, the sweep and the sync -
//! is `Engine::set_review_mode` in [`crate::engine`], which is where the
//! ordering argument lives; this module is the half that decides what an answer
//! would produce, so the plan and the fold can be held to one rule by sharing
//! one [`surviving_base`] rather than by two call sites agreeing to agree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crystalline_core::config::DomainEntry;
use crystalline_index::EngramDescriptor;
use serde_json::{Value, json};

use crate::engine::EngineError;

/// What one actor's drafts become when their domain stops reviewing changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldChoice {
    /// Write them into the folder the team shares: a rewrite becomes the file,
    /// a page only they had becomes a file the team has, and a deletion takes
    /// the file away.
    Fold,
    /// End them where they are. The folder never hears about them, and nothing
    /// brings them back.
    Discard,
}

/// Whether a change of review mode is being asked about or made.
///
/// Leaving review mode is the direction that needs an answer per actor, so the
/// two halves are not symmetric: a disable with no choices is the question, and
/// turning review ON carries no choices at all, since there are no drafts yet
/// for anybody to decide about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewModeConfirm {
    /// Answer what this would do, and write nothing.
    Preview,
    /// Make the change, with one [`FoldChoice`] per actor holding drafts.
    Confirmed {
        /// Each actor holding drafts, and what happens to them. Every actor the
        /// plan names has to be here, and nobody else.
        folds: Vec<(String, FoldChoice)>,
    },
}

/// One actor's drafts in a domain: the per-actor view the fold plan and the
/// removal gate are both drawn from.
pub(crate) struct ActorDrafts {
    /// Whose drafts these are.
    pub(crate) actor: String,
    /// Their rows in this domain, ordered by path, tombstones included.
    pub(crate) entries: Vec<crystalline_index::StoredEngram>,
}

/// The receipt of a domain that takes changes directly now, whether this
/// call is what made it so or found it that way.
pub(crate) fn left_json(
    domain: &str,
    folded: Vec<Value>,
    discarded: Vec<Value>,
    rooms_closed: usize,
) -> Value {
    json!({
        "domain": domain,
        "mode": "direct",
        "review": Value::Null,
        "applied": true,
        "folded": folded,
        "discarded": discarded,
        "rooms_closed": rooms_closed,
    })
}

/// The plan a preview answers with, and the shape Task 8's removal gate
/// reads the same rows through.
///
/// `conflict` on a draft is the choice-independent half: an address another
/// path already answers to, which no fold of that draft alone can avoid.
/// `contested_paths` is the other half, which depends on the answers: a path
/// two actors are drafting refuses only if both of them fold.
pub(crate) fn plan_json(
    domain: &str,
    entry: &DomainEntry,
    drafts: &[ActorDrafts],
    base: &[EngramDescriptor],
) -> Value {
    let mut by_path: HashMap<&str, Vec<&str>> = HashMap::new();
    for held in drafts {
        for draft in &held.entries {
            by_path
                .entry(draft.path.as_str())
                .or_default()
                .push(held.actor.as_str());
        }
    }
    let actors: Vec<Value> = drafts
        .iter()
        .map(|held| {
            // The folder as it would be if THIS actor folded and nobody
            // else did, which is the only address question a plan can
            // answer before the answers are in.
            let surviving = surviving_base(&[held], base);
            let rows: Vec<Value> = held
                .entries
                .iter()
                .map(|draft| {
                    let conflict = (!draft.tombstone)
                        .then(|| address_held_elsewhere(draft, &surviving))
                        .flatten();
                    json!({
                        "path": draft.path,
                        "permalink": draft.permalink,
                        "tombstone": draft.tombstone,
                        "conflict": conflict,
                    })
                })
                .collect();
            json!({
                "actor": held.actor,
                "entries": held.entries.len(),
                "drafts": rows,
            })
        })
        .collect();
    let mut contested: Vec<Value> = by_path
        .into_iter()
        .filter(|(_, actors)| actors.len() > 1)
        .map(|(path, mut actors)| {
            actors.sort_unstable();
            json!({ "path": path, "actors": actors })
        })
        .collect();
    contested.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    json!({
        "domain": domain,
        "mode": "direct",
        // What the domain says today, so a plan for a domain that has
        // already stopped reviewing does not claim it still does.
        "review": entry.is_overlay().then_some("overlay"),
        "applied": false,
        "actors": actors,
        "contested_paths": contested,
        "contested_addresses": contested_addresses(drafts),
    })
}

/// The addresses the folder would still answer to once the deletions in
/// `folding` had landed: permalink to path, over the base rows no folded
/// tombstone takes away.
///
/// **The one projection both halves of the address rule are asked of**, and
/// that is the point of it existing rather than each half computing its
/// own: the preview asks it per actor ("what if only they folded") to fill
/// in a draft's `conflict`, and [`collision`] asks it over
/// every folded actor to decide the refusal. Asked two different ways they
/// drifted, and the drift landed on the commonest flow there is - a rename
/// inside one overlay is a tombstone at the old path plus an entry at the
/// new one carrying the same address (`move_within_overlay`), so a
/// projection that did not subtract the tombstone called every rename a
/// collision in the plan and then folded it without complaint.
pub(crate) fn surviving_base<'a>(
    folding: &[&ActorDrafts],
    base: &'a [EngramDescriptor],
) -> HashMap<&'a str, &'a str> {
    let deleted: HashSet<&str> = folding
        .iter()
        .flat_map(|held| held.entries.iter())
        .filter(|draft| draft.tombstone)
        .map(|draft| draft.path.as_str())
        .collect();
    base.iter()
        .filter(|row| !deleted.contains(row.path.as_str()))
        .map(|row| (row.permalink.as_str(), row.path.as_str()))
        .collect()
}

/// The base engram, if any, that would still answer to this draft's address
/// at a different path once this actor's own deletions had landed - the
/// collision a fold of this actor's drafts alone cannot avoid.
pub(crate) fn address_held_elsewhere(
    draft: &crystalline_index::StoredEngram,
    surviving: &HashMap<&str, &str>,
) -> Option<String> {
    surviving
        .get(draft.permalink.as_str())
        .filter(|path| **path != draft.path)
        .map(|path| {
            format!(
                "the address '{}' already belongs to {path} in the folder the team shares",
                draft.permalink
            )
        })
}

/// The addresses two actors' different paths would both claim.
///
/// The other half of what a plan can say about addresses, and it is
/// choice-dependent where a draft's own `conflict` is not: neither draft is
/// in the folder for the other one's projection to find, and the paths
/// differ so `contested_paths` says nothing either. Without it a plan that
/// looked clean was refused at confirm time, which is exactly the
/// "never decided by omission" property the rest of this verb is built on.
pub(crate) fn contested_addresses(drafts: &[ActorDrafts]) -> Vec<Value> {
    let mut by_address: BTreeMap<&str, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for held in drafts {
        for draft in held.entries.iter().filter(|draft| !draft.tombstone) {
            let entry = by_address.entry(draft.permalink.as_str()).or_default();
            entry.0.insert(draft.path.as_str());
            entry.1.insert(held.actor.as_str());
        }
    }
    by_address
        .into_iter()
        // One path two actors are both drafting is `contested_paths`'
        // answer, not this one: at most one of them may be the file there
        // whatever address they give it.
        .filter(|(_, (paths, actors))| paths.len() > 1 && actors.len() > 1)
        .map(|(permalink, (paths, actors))| {
            json!({
                "permalink": permalink,
                "paths": paths.into_iter().collect::<Vec<_>>(),
                "actors": actors.into_iter().collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// One choice per actor holding drafts, refusing an actor left out and an
/// actor named who holds nothing here.
pub(crate) fn choices(
    domain: &str,
    drafts: &[ActorDrafts],
    folds: &[(String, FoldChoice)],
) -> std::result::Result<HashMap<String, FoldChoice>, EngineError> {
    let mut chosen: HashMap<String, FoldChoice> = HashMap::new();
    for (actor, choice) in folds {
        if chosen.insert(actor.clone(), *choice).is_some() {
            return Err(EngineError::Conflict(format!(
                "'{actor}' is named twice in this answer, once for each of two different \
                 things to do with the same drafts; say what happens to them once"
            )));
        }
    }
    let holding: BTreeSet<&str> = drafts.iter().map(|d| d.actor.as_str()).collect();
    let missing: Vec<&str> = holding
        .iter()
        .filter(|actor| !chosen.contains_key(**actor))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(EngineError::ConfirmationRequired(format!(
            "leaving review mode ends every private draft in domain '{domain}', and nothing \
             here says what happens to {}: their drafts live in this index alone. Say fold \
             (write them into the folder the team shares) or discard (end them) for each of \
             them, then answer again",
            missing.join(", ")
        )));
    }
    let strangers: Vec<&str> = chosen
        .keys()
        .map(String::as_str)
        .filter(|actor| !holding.contains(actor))
        .collect();
    if !strangers.is_empty() {
        let mut strangers = strangers;
        strangers.sort_unstable();
        return Err(EngineError::Conflict(format!(
            "nobody is drafting in domain '{domain}' as {}, so there is nothing there to fold \
             or discard; ask for the plan again and answer the actors it names",
            strangers.join(", ")
        )));
    }
    Ok(chosen)
}

/// The one address rule of a fold, asked once over the whole batch: what the
/// folder would hold if every fold in this answer landed, refused at the
/// first path or address two engrams would share.
///
/// Deliberately not [`crate::engine::Engine::refuse_permalink_held_elsewhere`], whose
/// tombstone exception is "a path THIS actor has deleted is free": that
/// holds for a draft written into one actor's own dimension and does not
/// transfer to a fold, where alice's deletion frees an address for bob only
/// if alice's deletion is being folded too. Here the deletions being folded
/// are exactly the ones that free anything.
pub(crate) fn collision(
    domain: &str,
    folding: &[&ActorDrafts],
    base: &[EngramDescriptor],
) -> Option<String> {
    // Two actors folding one path: whoever went second would be the file,
    // which is not an answer either of them gave.
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for held in folding {
        for draft in &held.entries {
            if let Some(first) = owner.insert(draft.path.as_str(), held.actor.as_str()) {
                return Some(format!(
                    "'{first}' and '{}' are both drafting {} in domain '{domain}', and only \
                     one of them can be the file: fold one of them and discard the other, or \
                     let them settle it between themselves first",
                    held.actor, draft.path
                ));
            }
        }
    }
    // What the folder would answer to afterwards: the projection every
    // half of this rule shares, plus every folded draft on top of it.
    let mut address = surviving_base(folding, base);
    for held in folding {
        for draft in &held.entries {
            if draft.tombstone {
                continue;
            }
            match address.insert(draft.permalink.as_str(), draft.path.as_str()) {
                Some(other) if other != draft.path => {
                    return Some(format!(
                        "folding '{}' drafted by {} into domain '{domain}' would give the \
                         address '{}' to a second engram: {other} already answers to it. One \
                         engram answers to one address, so give the draft an address of its \
                         own, or discard it",
                        draft.path, held.actor, draft.permalink
                    ));
                }
                _ => {}
            }
        }
    }
    None
}

/// Every actor's count as a surface reports it: `[{ actor, entries }]` in the
/// order the store answered, which is by actor.
///
/// One spelling for the removal's question, the status surfaces and the drafts
/// route, so a client that learns the shape on one of them can read all three.
/// Names and numbers only: a count of somebody's unshared work is coordination,
/// and the work itself is theirs until they share it.
pub(crate) fn counts_json(counts: &[(String, u64)]) -> Vec<Value> {
    counts
        .iter()
        .map(|(actor, entries)| json!({ "actor": actor, "entries": entries }))
        .collect()
}

/// What a status entry says about the drafts a reviewing domain holds: this
/// caller's own count for anybody, and everybody's for whoever holds the
/// domain.
///
/// The split is the membership rule, not a new one: a count of your own
/// unshared work is yours to know, and the coordination view - who else is
/// holding how much - belongs to the person who owns the domain and is
/// answerable for it. A member gets no `drafts` key rather than an empty one,
/// because an empty list would say "nobody else is drafting here", which is a
/// different and possibly wrong thing.
pub(crate) struct DraftView {
    /// Every actor's count, or `None` when the index could not be asked.
    counts: Option<Vec<(String, u64)>>,
    /// Whose status this is, so it can carry their own count. `None` is nobody
    /// in particular, who holds no drafts anywhere.
    actor: Option<String>,
    /// Whether this caller may see who else is drafting here.
    everyone: bool,
}

impl DraftView {
    pub(crate) fn new(
        counts: Option<Vec<(String, u64)>>,
        actor: Option<String>,
        everyone: bool,
    ) -> Self {
        Self {
            counts,
            actor,
            everyone,
        }
    }

    /// This caller's own count, or null when the index could not be asked.
    ///
    /// Null rather than zero for the reason `behind` is null when nothing
    /// probed it: "you are holding nothing here" and "this could not be read"
    /// are different answers, and only one of them is safe to act on.
    pub(crate) fn mine(&self) -> Value {
        let Some(counts) = &self.counts else {
            return Value::Null;
        };
        let Some(actor) = &self.actor else {
            return json!(0);
        };
        json!(
            counts
                .iter()
                .find(|(who, _)| who == actor)
                .map_or(0, |(_, entries)| *entries)
        )
    }

    /// Every actor's count, for a caller who may see it; `None` for one who
    /// may not, so the key is absent rather than empty.
    pub(crate) fn everyone(&self) -> Option<Value> {
        if !self.everyone {
            return None;
        }
        Some(match &self.counts {
            Some(counts) => json!(counts_json(counts)),
            None => Value::Null,
        })
    }
}

/// The one gate on a removal that would end somebody else's unshared work.
///
/// Leaving review mode asks what happens to each actor's drafts; a removal has
/// no fold to offer, so the only answer left is that they are being ended, and
/// naming each actor IS the answer. The caller's own drafts are left out of the
/// question: they are the one person in the room who already knows.
///
/// `counts` is `None` when the index could not be asked, which refuses on its
/// own. An unreadable count is not an empty one, and the branch deciding
/// whether somebody's only copy of their work is deleted must never read a
/// failure as "there was nothing there".
pub(crate) fn removal_choices(
    domain: &str,
    counts: Option<&[(String, u64)]>,
    own: Option<&str>,
    end_drafts: &[String],
) -> std::result::Result<(), EngineError> {
    let Some(counts) = counts else {
        return Err(EngineError::ConfirmationRequired(format!(
            "whether anybody holds private drafts in domain '{domain}' could not be read, and \
             unregistering it would end every one of them with nothing to bring them back. \
             Nothing was removed. Try again once the index answers"
        )));
    };
    let others: Vec<&(String, u64)> = counts
        .iter()
        .filter(|(actor, _)| Some(actor.as_str()) != own)
        .collect();
    let named: BTreeSet<&str> = end_drafts.iter().map(String::as_str).collect();
    let strangers: Vec<&str> = named
        .iter()
        .filter(|actor| !others.iter().any(|(who, _)| who == *actor))
        .copied()
        .collect();
    if !strangers.is_empty() {
        return Err(EngineError::Conflict(format!(
            "nobody else is drafting in domain '{domain}' as {}, so there is nothing of theirs \
             for this removal to end; ask for the removal preview again and name the actors it \
             reports",
            strangers.join(", ")
        )));
    }
    let missing: Vec<String> = others
        .iter()
        .filter(|(actor, _)| !named.contains(actor.as_str()))
        .map(|(actor, entries)| {
            let drafts = if *entries == 1 { "draft" } else { "drafts" };
            format!("{actor} ({entries} {drafts})")
        })
        .collect();
    if !missing.is_empty() {
        return Err(EngineError::ConfirmationRequired(format!(
            "domain '{domain}' holds private drafts that are not yours: {}. Unregistering it \
             ends them, and their work lives in this index alone, so nothing brings it back. \
             Repeat the removal naming each of them, once per person - 'end_drafts' over MCP, \
             '?end_drafts=' on the JSON API, '--end-drafts' at the command line",
            missing.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, u64)]) -> Vec<(String, u64)> {
        pairs
            .iter()
            .map(|(actor, entries)| ((*actor).to_string(), *entries))
            .collect()
    }

    /// A count nothing could read is not a count of zero, and the one branch
    /// that decides whether somebody's only copy of their work is deleted must
    /// never read a failure as "there was nothing there".
    ///
    /// A unit test because the failure it is about is an index that will not
    /// answer, which no integration fixture can provoke cheaply: the decision
    /// is a pure function of what the read returned, so it is asked here
    /// directly.
    #[test]
    fn an_unreadable_count_refuses_however_the_removal_was_confirmed() {
        for named in [Vec::new(), vec!["ada".to_string()]] {
            let err = removal_choices("eng", None, Some("owner"), &named)
                .expect_err("an unreadable count refuses, and nothing is removed");
            assert!(
                matches!(err, EngineError::ConfirmationRequired(_)),
                "it is the same class of refusal the unconfirmed purge raises: {err:?}"
            );
            assert!(
                err.to_string().contains("could not be read"),
                "and it says why: {err}"
            );
        }
    }

    /// Nobody is asked about their own drafts, and everybody else is.
    #[test]
    fn only_other_actors_have_to_be_named() {
        let held = counts(&[("ada", 2), ("owner", 1)]);
        removal_choices("eng", Some(&held), Some("owner"), &["ada".to_string()])
            .expect("the other actor is named, so the removal may go ahead");
        let err = removal_choices("eng", Some(&held), Some("owner"), &[])
            .expect_err("somebody else's drafts are not ended by omission");
        let text = err.to_string();
        assert!(
            text.contains("ada (2 drafts)") && !text.contains("owner"),
            "the refusal names the other actor and their count, and asks nothing about the \
             caller's own: {text}"
        );
        // A domain where only the caller is drafting asks nothing at all.
        removal_choices("eng", Some(&counts(&[("owner", 1)])), Some("owner"), &[])
            .expect("nobody is asked to confirm the ending of their own drafts");
        // And a count of one reads as one.
        let err = removal_choices("eng", Some(&counts(&[("ada", 1)])), Some("owner"), &[])
            .expect_err("one draft is still somebody's unshared work");
        assert!(err.to_string().contains("ada (1 draft)"), "{err}");
    }
}
