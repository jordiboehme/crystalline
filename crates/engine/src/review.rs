//! Review mode's plan and its fold: what leaving review mode would do to every
//! actor's private drafts, and the one address rule that decides whether it can
//! be done at all.
//!
//! A domain in review mode takes every write into its author's own draft, and
//! the folder the team shares changes only through a reviewed proposal. Coming
//! back out of that ends every draft one way or the other, so it is a question
//! before it is a change: `plan_json` answers what would happen and writes
//! nothing, and `choices` insists that the answer name every actor the plan
//! named and nobody else.
//!
//! Everything here is a pure function over rows somebody else read. The verb
//! itself - the gates, the config key, the file writes, the sweep and the sync -
//! is `Engine::set_review_mode` in [`crate::engine`], which is where the
//! ordering argument lives; this module is the half that decides what an answer
//! would produce, so the plan and the fold can be held to one rule by sharing
//! one `DomainView::address_map` rather than by two call sites agreeing to
//! agree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crystalline_core::config::DomainEntry;
use crystalline_index::EngramDescriptor;
use serde_json::{Value, json};

use crate::domain_view::DomainView;
use crate::engine::{DraftConflict, EngineError};

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
    /// Answer what this would do, and fold nothing. The convergence pass runs
    /// first, so a draft the team's change merges into is merged.
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
    /// Their files in this domain, ordered by path, deletion markers included.
    ///
    /// Beside the rows rather than folded into them, because the two are not
    /// one kind of thing wearing two hats: a row carries an address and a
    /// document, a file carries bytes and answers to nothing. Everything that
    /// counts draft changes counts both; everything that reasons about
    /// addresses reads [`ActorDrafts::entries`] alone.
    pub(crate) files: Vec<crate::overlay_files::FileEntry>,
    /// Whether this actor's files could all be listed.
    ///
    /// A files overlay nobody can read is not an empty one - the bytes are the
    /// only copy there is - so a fold of this actor refuses rather than
    /// silently folding the half that could be seen.
    pub(crate) files_unreadable: bool,
}

impl ActorDrafts {
    /// How many draft changes this actor is holding: rows plus files.
    ///
    /// The one arithmetic behind "3 draft changes, 1 of them a file", so the
    /// plan, the receipt and the removal gate cannot count differently.
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len() + self.files.len()
    }
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

/// What each actor's drafts in a fold plan stand against: the conflicts read
/// for each of them, the actors whose conflicts could not be read, and who is
/// asking, so a row of their own is said to them as theirs.
pub(crate) struct PlanConflicts<'a> {
    pub(crate) conflicts: &'a HashMap<String, Vec<DraftConflict>>,
    pub(crate) unreadable: &'a HashSet<String>,
    /// The names with no account any more, among those with conflicts.
    pub(crate) removed: &'a HashSet<String>,
    pub(crate) reader: Reader<'a>,
}

/// Who reads a plan or a refusal: their own drafts are said to them as theirs,
/// and the way out for a removed name depends on whether they are an admin.
#[derive(Clone, Copy)]
pub(crate) struct Reader<'a> {
    pub(crate) viewer: Option<&'a str>,
    pub(crate) admin: bool,
}

impl Reader<'_> {
    /// The way out of a removed name's conflicts that this reader can take.
    fn removed_way_out(&self) -> &'static str {
        if self.admin {
            "Choose discard, or end their drafts on the Users screen."
        } else {
            "Choose discard, or ask an admin to end their drafts on the Users screen."
        }
    }
}

/// The plan a preview answers with, and the shape Task 8's removal gate
/// reads the same rows through.
///
/// `conflict` on a draft is the choice-independent half: a draft that does
/// not fit the team's text, which only its author can settle, or an address
/// another path already answers to, which no fold of that draft alone can
/// avoid. A move's deletion half carries its draft's sentence too.
/// `contested_paths` is the other half, which depends on the answers: a path
/// two actors are drafting refuses only if both of them fold.
pub(crate) fn plan_json(
    domain: &str,
    entry: &DomainEntry,
    drafts: &[ActorDrafts],
    base: &[EngramDescriptor],
    standing: &PlanConflicts<'_>,
) -> Value {
    let mut by_path: HashMap<&str, Vec<&str>> = HashMap::new();
    for held in drafts {
        // Files are in here beside the rows because the question is the same
        // one: at most one person can be the thing standing at a path, and a
        // PNG two people drafted is as contested as a page two people drafted.
        for path in held
            .entries
            .iter()
            .map(|draft| draft.path.as_str())
            .chain(held.files.iter().map(|file| file.path.as_str()))
        {
            by_path.entry(path).or_default().push(held.actor.as_str());
        }
    }
    let actors: Vec<Value> = drafts
        .iter()
        .map(|held| {
            // The folder as it would be if THIS actor folded and nobody
            // else did, which is the only address question a plan can
            // answer before the answers are in.
            let surviving = DomainView::address_map(&[held], base);
            let unsettled: HashSet<&str> = standing
                .conflicts
                .get(&held.actor)
                .into_iter()
                .flatten()
                .flat_map(|c| std::iter::once(c.path.as_str()).chain(c.moved_from.as_deref()))
                .collect();
            let actor = held.actor.as_str();
            let sentence = if standing.reader.viewer == Some(actor) {
                "You have a conflict here. Settle it first.".to_string()
            } else if standing.removed.contains(actor) {
                format!(
                    "'{actor}' has no account any more, so nobody can settle this. {}",
                    standing.reader.removed_way_out()
                )
            } else {
                format!("'{actor}' has a conflict here. Ask them to settle it, or choose discard.")
            };
            let mut rows: Vec<Value> = held
                .entries
                .iter()
                .map(|draft| {
                    let conflict = if unsettled.contains(draft.path.as_str()) {
                        Some(sentence.clone())
                    } else {
                        (!draft.tombstone)
                            .then(|| address_held_elsewhere(draft, &surviving))
                            .flatten()
                    };
                    json!({
                        "path": draft.path,
                        "permalink": draft.permalink,
                        "tombstone": draft.tombstone,
                        "conflict": conflict,
                    })
                })
                .collect();
            // After the engram rows, and told apart by `kind` rather than by
            // position: an engram row keeps its exact shape, so a reader that
            // knows the old one goes on reading it, and a file carries no
            // `permalink` at all because it answers to no address. `conflict`
            // is null for the same reason - the only thing a plan can say is
            // in another draft's way is an address.
            rows.extend(held.files.iter().map(|file| {
                json!({
                    "path": file.path,
                    "kind": "file",
                    "tombstone": file.tombstone,
                    "conflict": unsettled
                        .contains(file.path.as_str())
                        .then(|| sentence.clone()),
                })
            }));
            let mut actor = json!({
                "actor": held.actor,
                "entries": held.entry_count(),
                "drafts": rows,
            });
            // Present only when it is true, so an actor whose whole overlay
            // could be read is exactly the shape they always were. An actor
            // whose files could not be listed is REPORTED rather than left out:
            // a plan that omitted them said there was nothing to decide over
            // work nobody could see, and leaving review mode then ended it. The
            // count beside it is what could be read, which is why the flag is a
            // fact of its own rather than a number.
            if held.files_unreadable
                && let Some(object) = actor.as_object_mut()
            {
                object.insert("files_unreadable".to_string(), json!(true));
            }
            // The same for an actor whose conflicts could not be read: a fold
            // of them refuses, and the plan says why before anyone answers,
            // in the refusal's own sentence for this reader.
            if standing.unreadable.contains(&held.actor)
                && let Some(object) = actor.as_object_mut()
            {
                object.insert("conflicts_unreadable".to_string(), json!(true));
                object.insert(
                    "conflicts_unreadable_reason".to_string(),
                    json!(unreadable_reason(&held.actor, standing.reader)),
                );
            }
            actor
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

/// The refusal of a fold while drafts that do not fit the team's text stand,
/// or `None` when every folding actor's drafts are settled.
///
/// Only the actors in `folding` are asked: discard ends a draft whatever it
/// holds, so a colleague's conflict never blocks the others once discard is
/// chosen for them. A name in `removed` has no account, so nobody can settle
/// its conflicts and the sentence names the ways out `reader` can take.
/// `recovering` is a fold repeated after one that stopped part way: part of
/// the domain is folded already, and what it wrote stays in the folder.
pub(crate) fn unsettled(
    domain: &str,
    folding: &[&ActorDrafts],
    conflicts: &HashMap<String, Vec<DraftConflict>>,
    removed: &HashSet<String>,
    reader: Reader<'_>,
    recovering: bool,
) -> Option<String> {
    let mut sentences = Vec::new();
    for held in folding {
        let Some(standing) = conflicts.get(&held.actor).filter(|c| !c.is_empty()) else {
            continue;
        };
        let actor = held.actor.as_str();
        let paths: Vec<&str> = standing.iter().map(|c| c.path.as_str()).collect();
        let paths = paths.join(", ");
        sentences.push(if reader.viewer == Some(actor) {
            format!(
                "You have conflicts in {paths}. Settle them first, or choose discard, which \
                 ends your text."
            )
        } else if removed.contains(actor) {
            format!(
                "'{actor}' has conflicts in {paths} and no account any more, so nobody can \
                 settle them. {}",
                reader.removed_way_out()
            )
        } else {
            format!(
                "'{actor}' has conflicts in {paths}. They can settle them, or you can choose \
                 discard, which ends their text."
            )
        });
    }
    if sentences.is_empty() {
        return None;
    }
    Some(match recovering {
        false => format!("Nothing in '{domain}' was folded. {}", sentences.join(" ")),
        true => format!(
            "Nothing more in '{domain}' was folded. {} An earlier fold stopped part way, and \
             what it wrote stays in the folder whatever you choose.",
            sentences.join(" ")
        ),
    })
}

/// The refusal of a fold when `actor`'s conflicts could not be read: their
/// drafts cannot be folded safely, and discard is the way that is left.
pub(crate) fn unreadable(
    domain: &str,
    actor: &str,
    reader: Reader<'_>,
    recovering: bool,
) -> String {
    let lead = match recovering {
        false => format!("Nothing in '{domain}' was folded."),
        true => format!("Nothing more in '{domain}' was folded."),
    };
    format!("{lead} {}", unreadable_reason(actor, reader))
}

/// Why `actor`'s drafts cannot be folded when their conflicts could not be
/// read, and the way out `reader` can take: the refusal's sentence without
/// its lead, which a plan carries on that actor before anybody answers.
pub(crate) fn unreadable_reason(actor: &str, reader: Reader<'_>) -> String {
    // Only an admin can read the daemon log the cause went to, or repair the
    // records under the state folder.
    let repair = match reader.admin {
        true => "repair the records the daemon log names and fold again",
        false => "ask an admin to repair the records and fold again",
    };
    if reader.viewer == Some(actor) {
        format!(
            "Your draft records could not be read, so your drafts cannot be folded safely. \
             Choose discard, which ends your text, or {repair}."
        )
    } else {
        format!(
            "The draft records of '{actor}' could not be read, so their drafts cannot be folded \
             safely. Choose discard for '{actor}', which ends their text, or {repair}."
        )
    }
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
///
/// `root` is the domain's folder, where two people folding `Zed.md` and `zed.md`
/// may be folding one file. The folder they land in answers that, and is only
/// asked about such a pair. One actor's own pair is a case-only move and is no
/// collision either way.
pub(crate) fn collision(
    domain: &str,
    folding: &[&ActorDrafts],
    base: &[EngramDescriptor],
    root: Option<&std::path::Path>,
) -> Option<String> {
    // Two actors folding one path: whoever went second would be the file,
    // which is not an answer either of them gave.
    let mut owner: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for held in folding {
        // The rows and the files share one map, so one path two actors are
        // folding is refused in one sentence whichever kind of thing stands
        // there - and a page and a file can never collide with each other,
        // since an attachment path is never an engram path.
        for path in held
            .entries
            .iter()
            .map(|draft| draft.path.as_str())
            .chain(held.files.iter().map(|file| file.path.as_str()))
        {
            let same_name = owner
                .entry(crystalline_core::fold_path_case(path))
                .or_default();
            for &(first, first_path) in same_name.iter() {
                if first_path == path {
                    return Some(format!(
                        "'{first}' and '{}' are both drafting {path} in domain '{domain}', and \
                         only one of them can be the file: fold one of them and discard the \
                         other, or let them settle it between themselves first",
                        held.actor
                    ));
                }
                let may_be_one_file = root.is_some_and(|root| {
                    crate::engine::folder_may_fold_case(root, first_path)
                        || crate::engine::folder_may_fold_case(root, path)
                });
                if first != held.actor && may_be_one_file {
                    return Some(format!(
                        "'{first}' is drafting {first_path} and '{}' is drafting {path} in \
                         domain '{domain}'. The folder they land in may treat those two names \
                         as one file, so only one of them can land: fold one of them and \
                         discard the other, or let them settle it between themselves first",
                        held.actor
                    ));
                }
            }
            same_name.push((held.actor.as_str(), path));
        }
    }
    // What the folder would answer to afterwards: the projection every
    // half of this rule shares, plus every folded draft on top of it.
    let mut address = DomainView::address_map(folding, base);
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
    ///
    /// Absent too when the index could not be counted, rather than a null where
    /// a list belongs: the count beside it is already null and says so in
    /// words, and a reader that has to tell "not sent to you" from "there is
    /// nothing here" should never also have to tell either from "this key is
    /// sometimes not a list".
    pub(crate) fn everyone(&self) -> Option<Value> {
        if !self.everyone {
            return None;
        }
        self.counts
            .as_ref()
            .map(|counts| json!(counts_json(counts)))
    }
}

/// The one gate on a removal that would end somebody else's unshared work.
///
/// Leaving review mode asks what happens to each actor's drafts; a removal has
/// no fold to offer, so the only answer left is that they are being ended, and
/// naming each actor IS the answer. The caller's own drafts are left out of the
/// question - they are the one person in the room who already knows - but not
/// out of the answer: naming yourself is taken and changes nothing, because the
/// preview and the confirmation question both list you among the actors holding
/// drafts, and an answer read off that list must not be refused.
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
        // The caller's own name is never a stranger, whether they hold drafts
        // here or not. The preview lists every actor holding drafts, the caller
        // among them, and the confirmation question reads that list out - so an
        // answer that echoes the list back has to be served, or the refusal
        // would instruct the very thing it refuses. It is also what leaving
        // review mode asks for, and one verb telling an agent to name itself
        // while its neighbour refuses the same answer is two rules for one
        // question.
        .filter(|actor| Some(**actor) != own && !others.iter().any(|(who, _)| who == *actor))
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
             '?end_drafts=' on the JSON API, '--end-drafts' at the command line. Your own \
             drafts go with the domain either way: naming yourself as well is taken and \
             changes nothing",
            missing.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held_by(actor: &str) -> ActorDrafts {
        ActorDrafts {
            actor: actor.to_string(),
            entries: Vec::new(),
            files: Vec::new(),
            files_unreadable: false,
        }
    }

    fn conflict_at(path: &str) -> DraftConflict {
        DraftConflict {
            id: "d0000000".to_string(),
            path: path.to_string(),
            kind: crate::draft_merge::DraftConflictKind::EditEdit,
            moved_from: None,
            file: false,
            other: None,
        }
    }

    /// A person who cannot read the daemon log is pointed at an admin, never
    /// at the log.
    #[test]
    fn an_unreadable_list_points_a_non_admin_at_an_admin() {
        let reader = Reader {
            viewer: Some("jo"),
            admin: false,
        };
        assert_eq!(
            unreadable("eng", "ada", reader, false),
            "Nothing in 'eng' was folded. The draft records of 'ada' could not be read, so \
             their drafts cannot be folded safely. Choose discard for 'ada', which ends their \
             text, or ask an admin to repair the records and fold again."
        );
        let own = unreadable("eng", "jo", reader, false);
        assert!(!own.contains("log"), "{own}");
        assert!(own.contains("ask an admin"), "{own}");
    }

    /// The plan's sentence for an actor whose conflicts could not be read is
    /// the refusal's without its lead: their own to the actor, a colleague's
    /// to anyone else, and the repair the reader can do.
    #[test]
    fn an_unreadable_actor_reason_is_the_refusal_without_its_lead() {
        let as_jo = |admin| Reader {
            viewer: Some("jo"),
            admin,
        };
        assert_eq!(
            unreadable_reason("ada", as_jo(false)),
            "The draft records of 'ada' could not be read, so their drafts cannot be folded \
             safely. Choose discard for 'ada', which ends their text, or ask an admin to repair \
             the records and fold again."
        );
        assert_eq!(
            unreadable_reason("jo", as_jo(false)),
            "Your draft records could not be read, so your drafts cannot be folded safely. \
             Choose discard, which ends your text, or ask an admin to repair the records and \
             fold again."
        );
        assert_eq!(
            unreadable_reason("ada", as_jo(true)),
            "The draft records of 'ada' could not be read, so their drafts cannot be folded \
             safely. Choose discard for 'ada', which ends their text, or repair the records the \
             daemon log names and fold again."
        );
        assert_eq!(
            unreadable("eng", "ada", as_jo(true), false),
            format!(
                "Nothing in 'eng' was folded. {}",
                unreadable_reason("ada", as_jo(true))
            )
        );
    }

    /// A domain owner who is not an admin cannot end a removed name's drafts,
    /// so their refusal names the admin; an admin reads the way they can take.
    #[test]
    fn a_removed_names_way_out_is_one_the_reader_can_take() {
        let ada = held_by("ada");
        let conflicts = HashMap::from([("ada".to_string(), vec![conflict_at("plan.md")])]);
        let removed = HashSet::from(["ada".to_string()]);
        let said = |admin| {
            unsettled(
                "eng",
                &[&ada],
                &conflicts,
                &removed,
                Reader {
                    viewer: Some("jo"),
                    admin,
                },
                false,
            )
            .unwrap()
        };
        assert_eq!(
            said(false),
            "Nothing in 'eng' was folded. 'ada' has conflicts in plan.md and no account any \
             more, so nobody can settle them. Choose discard, or ask an admin to end their \
             drafts on the Users screen."
        );
        assert!(
            said(true).ends_with("Choose discard, or end their drafts on the Users screen."),
            "{}",
            said(true)
        );
    }

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

    /// Your own name is an answer, never a question.
    ///
    /// The preview lists every actor holding drafts, the caller included, and
    /// the MCP confirmation question reads that list out. An answer that echoes
    /// it back has to be accepted, or the refusal would instruct the very thing
    /// it refuses; and leaving review mode, which this mirrors, wants the caller
    /// named. Both shapes are good.
    #[test]
    fn naming_yourself_is_accepted_and_never_required() {
        let held = counts(&[("ada", 2), ("owner", 1)]);
        for answer in [
            vec!["ada".to_string()],
            vec!["ada".to_string(), "owner".to_string()],
        ] {
            removal_choices("eng", Some(&held), Some("owner"), &answer)
                .expect("both shapes of the answer name every actor but the caller");
        }
        // Even where the caller holds nothing here: a client that names the
        // session's own actor out of habit is not asking about a stranger.
        removal_choices(
            "eng",
            Some(&counts(&[("ada", 1)])),
            Some("owner"),
            &["ada".to_string(), "owner".to_string()],
        )
        .expect("your own name is never the stranger");
    }

    /// An owner whose index could not be counted is sent no per-actor list at
    /// all, rather than a null where an array belongs.
    #[test]
    fn an_uncountable_index_sends_no_per_actor_list() {
        let view = DraftView::new(None, Some("owner".to_string()), true);
        assert_eq!(view.mine(), Value::Null, "the count says it is unknown");
        assert!(
            view.everyone().is_none(),
            "and the key that would carry a list is absent, not null"
        );
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
