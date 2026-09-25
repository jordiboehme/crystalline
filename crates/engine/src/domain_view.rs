//! One view of what is real to an actor in one domain: the folder the team
//! reviewed, with that reader's own drafts laid over it.
//!
//! A domain that reviews changes holds two things at once. The folder on disk
//! (or, for a virtual domain, the `actor = ''` rows) is what the team reviewed
//! and is the same for everybody. Beside it every member has their own draft
//! overlay - a full row per path in their own dimension, a tombstone where they
//! have deleted something - and what they read is the first laid over the
//! second. [`DomainView`] is that composition, named once, so the fold, a
//! share, the sweep, a browse level, a graph traversal and every write verb ask
//! one thing rather than deriving the same three edits six ways. The three
//! edits, in the words every operation here keeps: a path this reader has
//! deleted is absent, a path they are drafting is described by their own row,
//! and a draft at a path no file holds is a row like any other.
//!
//! **`actor: None` is the base view**, which is a domain that takes changes
//! directly or a reader with no identity of their own, and every operation
//! short-circuits on it in its first line. That is what keeps a direct domain
//! byte for byte what it was before the dimension existed: there is no second
//! implementation of the short-circuit and no vtable in front of it, because
//! there are exactly two views and they differ by one optional field rather
//! than by behaviour.
//!
//! **The registered-set screen composes ahead of the actor dimension, never
//! behind it**, and here that is a constructor argument rather than a doc
//! comment: every constructor takes the screen its caller already computed (the
//! `hidden` set from [`Engine::hidden_for`] for a read, [`Engine::refuse_hidden_domain`]
//! for a write), so a view cannot be built without the screen having run. A
//! draft in a domain this reader may not see, and a draft in a domain this
//! instance has no registration for, are both absent before whose-draft-is-it
//! is ever asked. An internal pass that answers to no caller at all - the
//! convergence a pull runs, a withdrawal, the fold - screens nothing and says
//! so in code by passing an empty set, rather than by having somewhere to skip
//! the argument.
//!
//! **One question with two answers, named here so a third never appears.** A
//! search or a similarity probe spans domains and carries ONE actor key for the
//! whole query (`SearchQuery.actor`, resolved from the scope rather than
//! per domain): a domain nobody drafts in holds no overlay row, so the screen
//! collapses to the base there anyway, and deriving the key per domain would
//! let one domain's mode decide another domain's answer. That is deliberate and
//! it is the only other place in the tree that answers "whose rows is this
//! caller entitled to".
//!
//! [`crate::review::DraftView`] carries the word "view" for a different
//! meaning and is not one of these: it renders per-actor counts for a status
//! payload and projects nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use crystalline_core::emit::{set_frontmatter_field, touch_generated};
use crystalline_core::{CrystallineUrl, parse_engram};
use crystalline_index::{
    AttachmentRow, BrowseLevel, DomainId, EngramDescriptor, EngramId, EngramRecord, EngramSummary,
    OutboundRef, RecentFilter, StoredEngram,
};
use crystalline_remote::state::{self, BaseStamp};
use serde_json::{Value, json};

use crate::engine::{
    ContentSource, Engine, EngineError, EngramText, OVERLAY_NEEDS_IDENTITY, Result, asset_claim,
    folder_slash_lower, is_within_domain, join_rel, note_unmirrored, overlay_descriptor,
    overwrite_from_draft, sha256_hex, unmirrored, virtual_stamp,
};
use crate::params::MoveParams;
use crate::review::ActorDrafts;
use crate::share_staging::{OVERLAY_STAGING_DIR, OverlayStaging, write_staged_file};

/// What one reader sees in one domain: the folder the team reviewed, with that
/// reader's own drafts laid over it.
#[doc(hidden)]
pub struct DomainView<'a> {
    /// The engine the substrates live behind. A view is a lens, never an owner:
    /// it borrows for the length of one verb.
    engine: &'a Engine,
    /// Which domain this is a view of.
    domain: String,
    /// The base substrate as the registered-set screen returned it: the folder
    /// for a file domain, the database for a virtual one.
    ///
    /// `None` only on a write view of a domain that takes changes directly.
    /// Resolving one eagerly there would move an unregistered domain's
    /// [`EngineError::UnknownDomain`] ahead of the refusals each write verb
    /// makes in its own order, and no operation reads a substrate on a view
    /// whose actor is `None` anyway.
    base: Option<ContentSource>,
    /// Whose drafts stand over the base here. `None` is the base view: a domain
    /// that takes changes directly, or a reader with no identity of their own.
    actor: Option<String>,
    /// Whose draft this view is a JOIN into, when it is one: a caller working
    /// inside somebody else's draft through a share-link they redeemed and a
    /// join they opened. `None` on every ordinary view, which is nearly all of
    /// them.
    ///
    /// Equal to `actor` by construction wherever it is set - a joined view IS
    /// the owner's view, built through [`DomainView::for_actor`] - and kept
    /// beside it rather than inferred from it because it is a different fact:
    /// `actor` says whose rows these are, and this says that the caller is not
    /// that person. Two things read it: the files seam, which passes it to
    /// [`crate::overlay_files::target_actor`] so the one function that answers
    /// "whose files overlay does this write land in" answers it from the join
    /// rather than from an inference, and the receipt, which tells the caller
    /// whose draft their work landed in.
    joined: Option<String>,
}

impl<'a> DomainView<'a> {
    /// The base view: the folder the team reviewed and nothing else.
    ///
    /// What a reader with no identity sees, and what the collab seam reads a
    /// room's document through - see [`Engine::engram_text`], whose callers
    /// pass one of these.
    pub(crate) fn base(
        engine: &'a Engine,
        domain: &str,
        hidden: &HashSet<String>,
    ) -> Result<DomainView<'a>> {
        Ok(DomainView {
            engine,
            domain: domain.to_string(),
            base: Some(engine.content_source_scoped(domain, hidden)?),
            actor: None,
            joined: None,
        })
    }

    /// This reader's view.
    ///
    /// Infallible with respect to identity: a reader with no identity of their
    /// own gets the base view, which is a complete and correct answer rather
    /// than a denied one. Deliberately asymmetric with [`DomainView::for_write`],
    /// which does refuse - if both returned the same shape a later change could
    /// propagate a write refusal onto a read path, and
    /// [`OVERLAY_NEEDS_IDENTITY`] says "this domain reviews changes before they
    /// land", which is a fact about a domain a screened-out reader must not
    /// learn exists.
    #[doc(hidden)]
    pub fn for_read(
        engine: &'a Engine,
        domain: &str,
        hidden: &HashSet<String>,
        scope: &crate::scope::Scope,
    ) -> Result<DomainView<'a>> {
        let base = engine.content_source_scoped(domain, hidden)?;
        let actor = if engine.reviews_changes(domain) {
            crate::scope::overlay_actor(scope)
        } else {
            None
        };
        Ok(DomainView {
            engine,
            domain: domain.to_string(),
            base: Some(base),
            actor,
            joined: None,
        })
    }

    /// One named actor's view, for the surfaces that answer ABOUT somebody
    /// rather than TO them.
    ///
    /// Its callers, by name and exhaustively: the fold plan and the fold
    /// ([`Engine::set_review_mode`] through [`Engine::overlay_actor_drafts`]),
    /// the removal gate, a share resolved through
    /// [`crate::engine::ShareActor`], a withdrawal, a conflict resolution, and
    /// the convergence pass a pull runs - which names an actor without anybody
    /// having asked it a question. **No read verb may reach it**: a read
    /// request that did would answer one reader with another reader's drafts,
    /// which is the single worst failure this mode can have, and
    /// `another_actors_view_is_reached_only_by_the_owner_gated_surfaces` in
    /// crates/service/tests/overlay/overlay_domains.rs scans for exactly that.
    pub(crate) fn for_actor(
        engine: &'a Engine,
        domain: &str,
        hidden: &HashSet<String>,
        actor: &str,
    ) -> Result<DomainView<'a>> {
        Ok(DomainView {
            engine,
            domain: domain.to_string(),
            base: Some(engine.content_source_scoped(domain, hidden)?),
            actor: Some(actor.to_string()),
            joined: None,
        })
    }

    /// Which domain this is a view of.
    pub(crate) fn domain(&self) -> &str {
        &self.domain
    }

    /// Whose drafts stand over the base here, or `None` for the base view.
    pub(crate) fn actor(&self) -> Option<&str> {
        self.actor.as_deref()
    }

    /// The base substrate, for the one operation that reads the domain's own
    /// text at a path this view has replaced. `None` is unreachable from an
    /// actor view - see the field.
    fn substrate(&self) -> Option<&ContentSource> {
        self.base.as_ref()
    }

    /// The actor key a write acts under, refusing the base view the way
    /// [`DomainView::for_write`] already refused it - unreachable, because a
    /// write view with no actor never reaches an overlay arm.
    pub(crate) fn writing_actor(&self) -> Result<&str> {
        self.actor
            .as_deref()
            .ok_or_else(|| EngineError::Refused(OVERLAY_NEEDS_IDENTITY.to_string()))
    }
    /// This writer's view: whose draft the write joins, or the base view when
    /// the domain takes changes directly and the write goes to the folder or
    /// the database as it always did.
    ///
    /// The one place review mode turns into a routing decision, so every write
    /// verb asks it the same way and a verb added later inherits the rule
    /// instead of having to remember it. [`EngineError::Refused`] with
    /// [`OVERLAY_NEEDS_IDENTITY`] when the domain reviews changes and the
    /// caller has no identity: the alternative is a write falling through onto
    /// reviewed truth, which is the one outcome this whole mode exists to
    /// prevent.
    ///
    /// **The registered-set screen composes ahead of the actor dimension here
    /// too, and on this side it is load bearing twice over.** A write that
    /// routed would put a stranger's draft into a domain they may not see; and
    /// the refusal itself says "this domain reviews changes before they land",
    /// which is a fact about a domain they must not learn exists. So a domain
    /// [`Engine::hidden_for`] hides is answered here exactly as a domain nobody
    /// registered, by the same [`Engine::refuse_hidden_domain`] every read
    /// goes through, before either answer below can be reached.
    ///
    /// **A direct domain never reaches that screen**, and that is deliberate
    /// rather than an oversight: this function answers `None` for it on the
    /// first line, which is what it answered before review mode existed, so
    /// every direct write behaves byte for byte as it always has and keeps
    /// relying on the surface gate in front of it (MCP `refuse_unwritable`,
    /// REST `require_domain_write`) exactly as its neighbours do.
    pub(crate) async fn for_write(
        engine: &'a Engine,
        name: &str,
        scope: &crate::scope::Scope,
    ) -> Result<DomainView<'a>> {
        let seen = |actor: Option<String>| DomainView {
            engine,
            domain: name.to_string(),
            base: engine.content_source(name).ok(),
            actor,
            joined: None,
        };
        if !engine.reviews_changes(name) {
            return Ok(seen(None));
        }
        engine.refuse_hidden_domain(name, scope).await?;
        match crate::scope::overlay_actor(scope) {
            Some(actor) => Ok(seen(Some(actor))),
            None => Err(EngineError::Refused(OVERLAY_NEEDS_IDENTITY.to_string())),
        }
    }

    /// This writer's view when the write is being made **inside somebody
    /// else's draft**: the owner's overlay, not the caller's.
    ///
    /// The one place a join turns into a routing decision, exactly as
    /// [`DomainView::for_write`] is the one place review mode does, and it is
    /// deliberately the same shape: a verb that can be driven from inside a
    /// join asks this instead of that, and everything downstream of it sees an
    /// ordinary actor view.
    ///
    /// **This is the one caller allowed to build another actor's view from a
    /// write path**, and `another_actors_view_is_reached_only_by_the_owner_gated_surfaces`
    /// in crates/service/tests/overlay/overlay_domains.rs names it. What makes that
    /// safe is that the join is not something a caller asserts: it is a record
    /// this process minted, when an account presented a share-link its author
    /// minted on that very draft, and it names the owner rather than taking
    /// one from the request. Three things still stand between it and a write:
    /// the caller's account is checked against the join on every use (see
    /// [`crate::join::Joins::get`]), the domain has to be one this caller may
    /// see at all (the hidden-domain screen below, for the reason `for_write`
    /// gives), and the verb that called this checks that the path it resolved
    /// is the path the join was opened for.
    ///
    /// A join naming another domain, or one into a domain that has since
    /// stopped reviewing changes, is not an error and not a refusal: it is
    /// simply not a join into THIS write, so the ordinary view answers. A
    /// domain that left review mode has no drafts left to be inside of, and
    /// `Engine::end_domain_grants` has already ended the join; this is the
    /// belt to that braces.
    pub(crate) async fn for_write_joined(
        engine: &'a Engine,
        name: &str,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<DomainView<'a>> {
        let joined = join.filter(|j| j.domain == name && engine.reviews_changes(name));
        let Some(join) = joined else {
            return DomainView::for_write(engine, name, scope).await;
        };
        engine.refuse_hidden_domain(name, scope).await?;
        let mut view = DomainView::for_actor(engine, name, &HashSet::new(), &join.owner)?;
        view.joined = Some(join.owner.clone());
        Ok(view)
    }

    /// Whose draft this view is a join into, or `None` on every ordinary one.
    pub(crate) fn joined(&self) -> Option<&str> {
        self.joined.as_deref()
    }

    /// What `actor` sees at `path`: their own draft when they hold one, the
    /// base text otherwise, and `None` when their own tombstone deletes it or
    /// nothing stands there at all.
    ///
    /// The text every overlay write compares its `expected_checksum` against
    /// and every overlay edit applies to, which is why it is one function: a
    /// first edit has to read the base (there is no draft yet) and every later
    /// one has to read the draft (or the second edit conflicts against the
    /// first).
    pub(crate) async fn text_at(
        &self,
        base: &ContentSource,
        desc: &EngramDescriptor,
    ) -> Result<Option<String>> {
        let Some(actor) = self.actor.as_deref() else {
            return self.engine.load_content(base, desc).await.map(Some);
        };
        let held = {
            let store = self.engine.store();
            let store = store.lock().await;
            store
                .overlay_entry(desc.domain_id, actor, &desc.path)
                .await?
        };
        match held {
            Some(entry) if entry.tombstone => Ok(None),
            Some(entry) => Ok(Some(entry.content)),
            None => self.engine.load_content(base, desc).await.map(Some),
        }
    }

    /// Write one actor's draft of a path: the row, its chunks and its mirror.
    ///
    /// The single place a draft is created, so every verb that routes lands the
    /// same shape and the shape agrees with what the journal restore writes
    /// back after a wipe. A draft row carries the WHOLE document in its
    /// `content` column, frontmatter included, exactly as a virtual domain's
    /// rows do: no file on disk holds a draft, so the row is the only place the
    /// document lives and a body-only row would lose the frontmatter for good.
    ///
    /// **The row goes down before the mirror, and a mirror that fails does not
    /// unsay the row.** The answer is `Some(warning)`: the write landed, and
    /// this machine could not copy it where a `reindex --wipe` would find it.
    ///
    /// The ordering is what keeps the restore honest under Task 2's rule that
    /// store rows win. A mirror with no row is a GAP, so the next restore
    /// fills it - which would mean a write the caller was told had failed
    /// appearing as a draft later, and, for a tombstone, a deletion taking
    /// effect after the fact. A row with no mirror is not a gap, so the restore
    /// does nothing with it: the journal never holds anything the index did not
    /// accept, and the only loss is the one a wipe takes, which is exactly what
    /// the warning names.
    ///
    /// Reporting it as a failure instead would be worse than silent, because
    /// three callers act on that answer: a split would delete the engram
    /// holding the observations it just moved, a move's rollback would undo a
    /// move that happened, and a delete would be unretryable - its next attempt
    /// answering "no engram" against the tombstone it claims it did not write.
    /// The state directory is resolved before the row is ever written (in
    /// [`Engine::put_overlay_entry`], which is the half that writes), so an
    /// engine with no journal at all still refuses before it writes a row it
    /// could never mirror.
    ///
    /// **The address is checked here**, which is what makes this the one place
    /// a draft can be created: see
    /// [`Engine::refuse_permalink_held_elsewhere`], which every verb that
    /// routes through this writer inherits.
    pub(crate) async fn write(
        &self,
        domain_id: DomainId,
        path: &str,
        text: &str,
    ) -> Result<Option<String>> {
        let record = Engine::overlay_record(path, text)?;
        self.refuse_address_held_elsewhere(domain_id, &record.permalink, path, None)
            .await?;
        self.put(domain_id, path, record).await
    }

    /// [`Engine::write_overlay_entry`] without the address check, for the one
    /// caller that must never be refused: a move's rollback.
    ///
    /// The rollback puts back a draft this actor was already holding a moment
    /// ago, so the state it restores is one the check had already allowed, and
    /// the text it restores lives in that row and nowhere else - a refusal
    /// there would be the move losing the draft rather than not making it.
    /// [`Engine::move_within_overlay`] asks the check its own question before
    /// it writes anything at all, so the rule still holds for the move.
    pub(crate) async fn write_unchecked(
        &self,
        domain_id: DomainId,
        path: &str,
        text: &str,
    ) -> Result<Option<String>> {
        let record = Engine::overlay_record(path, text)?;
        self.put(domain_id, path, record).await
    }

    /// Refuse a draft that would answer to an address another path already
    /// answers to.
    ///
    /// The overlay dimension relaxes what the index enforces. The unique index
    /// is `(domain, permalink, actor)`, so `(team, plan, alice)` and
    /// `(team, plan, "")` are two different rows and the database says nothing
    /// about a draft of `notes.md` whose frontmatter now reads `permalink:
    /// plan` while the team's `plan.md` holds that address. Nothing downstream
    /// can carry those two rows: a search merges its hits by permalink and
    /// drops one of them silently, and a draft holding an address the reviewed
    /// folder already spends could never be folded back into that folder,
    /// since the base rows do refuse it there. So the write path keeps the
    /// rule, in the words the import path keeps it in
    /// (`permalink '...' already exists at another path`).
    ///
    /// **What counts as a holder is this actor's own view of the domain**,
    /// which is the only view their drafts live in: their own drafts, and the
    /// base rows their own tombstone has not deleted. A path they have deleted
    /// holds nothing for them, exactly as it holds no engram for them - the
    /// same reading [`Engine::write_engram_as`] already applies to a name - and
    /// a move depends on it, since a move tombstones the source before it
    /// writes the destination and the two carry one permalink between them.
    ///
    /// `vacating` is the path a caller is about to empty in the same breath: a
    /// move's source, which still holds the address at the moment the question
    /// is asked and will not hold it by the time the destination lands. It is
    /// the move's alone and no other verb may pass it - every other write
    /// leaves whatever it found standing, so a path named here that keeps its
    /// row is the rule quietly switched off for one address.
    pub(crate) async fn refuse_address_held_elsewhere(
        &self,
        domain_id: DomainId,
        permalink: &str,
        path: &str,
        vacating: Option<&str>,
    ) -> Result<()> {
        let actor = self.writing_actor()?;
        let domain = self.domain.as_str();
        let (entries, base) = {
            let store = self.engine.store();
            let store = store.lock().await;
            let entries = store.overlay_entries(domain_id, actor).await?;
            // `find_engram` answers a title as well as a permalink, and a title
            // is not an address anybody holds; an exact permalink sorts first,
            // so filtering the answer hides no real holder behind a namesake.
            let base = store
                .find_engram(domain, permalink)
                .await?
                .filter(|found| found.permalink == permalink);
            (entries, base)
        };
        let elsewhere = |at: &str| at != path && Some(at) != vacating;
        let held_at = entries
            .iter()
            .find(|entry| {
                !entry.tombstone && entry.permalink == permalink && elsewhere(&entry.path)
            })
            .map(|entry| entry.path.clone())
            .or_else(|| {
                base.filter(|found| {
                    elsewhere(&found.path)
                        && !entries
                            .iter()
                            .any(|entry| entry.tombstone && entry.path == found.path)
                })
                .map(|found| found.path)
            });
        if let Some(at) = held_at {
            return Err(EngineError::Conflict(format!(
                "permalink '{permalink}' already exists at another path ({at}) in domain \
                 '{domain}'; one engram answers to one address, so give this draft an address of \
                 its own, or draft the change to '{at}' instead"
            )));
        }
        Ok(())
    }

    /// The row and the mirror of a draft, once the address has been settled.
    pub(crate) async fn put(
        &self,
        domain_id: DomainId,
        path: &str,
        record: EngramRecord,
    ) -> Result<Option<String>> {
        let actor = self.writing_actor()?;
        let domain = self.domain.as_str();
        let state_dir = self.engine.journal_state_dir()?;
        self.engine
            .commit_overlay_row(domain_id, actor, &record)
            .await?;
        let warning = match crate::overlay_journal::journal_write(
            &state_dir,
            domain,
            actor,
            path,
            &record.content,
        ) {
            Ok(()) => None,
            Err(e) => Some(unmirrored(domain, actor, path, &e, false)),
        };
        if let Some(text) = &warning {
            tracing::warn!(domain, actor, path, "{text}");
        }
        self.engine.nudge_embed();
        Ok(warning)
    }

    /// Drop one actor's draft at a path, row and mirror together, so a verb
    /// that undoes a draft leaves nothing for a later restore to resurrect.
    ///
    /// **The mirror goes first here**, which is the opposite order to the two
    /// writers above and is the same rule read from the other end: a mirror
    /// that outlived its row is a gap the next restore fills, so clearing the
    /// row first and failing on the mirror would resurrect a draft its author
    /// dropped. Failing on the mirror before the row has moved refuses a call
    /// that did nothing, which is the honest answer and the retryable one.
    ///
    /// **This is also where a draft's share-links and joins end**, and it is
    /// the one place they can be ended once rather than at each verb. Every
    /// way an overlay row is taken away passes through here or through
    /// [`DomainView::drop_mid_move`] beside it - the discard of a domain's
    /// drafts on leaving review mode, the per-path discard of local changes
    /// ([`Engine::discard_local_changes`]), the fold, a withdrawal
    /// ([`Engine::revert_into_overlay`]), a conflict resolution
    /// ([`Engine::resolve_in_overlay`]), a settled convergence and the rename
    /// convergence performs when the base carried the draft along
    /// ([`Engine::move_draft_with_the_base`]), which never touches the move
    /// verb at all - so a verb added later inherits the ending instead of
    /// having to remember it. A grant lasts exactly as long as the thing it
    /// grants, and a row left live springs back onto whatever its author
    /// drafts at that path next.
    ///
    /// The WRITER above is deliberately not such a seam. A draft being saved
    /// is the same draft, and ending its links on every keystroke would mean a
    /// grant that survived only until its author next typed. Two paths replace
    /// a draft rather than removing it - the delete that stands a tombstone
    /// over a base row, and the move that does the same at its source - and
    /// each ends the grants itself, beside its call to this.
    ///
    /// Ended after the transaction commits, never inside it: the accounts
    /// database is a different store behind a different lock, and a row that
    /// is gone is what makes ending its grants the truth.
    pub(crate) async fn drop(&self, domain_id: DomainId, path: &str) -> Result<()> {
        let actor = self.writing_actor()?.to_string();
        self.clear_row(domain_id, path).await?;
        self.engine
            .end_draft_grants(self.domain.as_str(), &actor, path)
            .await;
        Ok(())
    }

    /// The same removal with the ending **left to the caller**, for the one
    /// shape that needs it: the source half of a move.
    ///
    /// A move is two writes, and until the second one lands the move has not
    /// happened - the source goes back to exactly what this actor held. So
    /// ending the links at the removal would end them on the way to not
    /// happening: a destination that could not be written would leave its
    /// author's page where it was and their share-link revoked, with whoever
    /// was inside the draft put out of it, over a move nobody made.
    ///
    /// Both movers end the links themselves once they know the move happened,
    /// and there is no third caller. The deferred-removal guard in
    /// crates/service/tests/overlay/overlay_domains.rs is what keeps it that way: a
    /// caller that took a row away and ended nothing would leave a link
    /// standing on a draft that is not there.
    pub(crate) async fn drop_mid_move(&self, domain_id: DomainId, path: &str) -> Result<()> {
        self.clear_row(domain_id, path).await
    }

    /// The removal itself: the mirror, the row and this actor's edges onto it.
    ///
    /// Private, and the only place [`Store::clear_overlay_entry`] is called
    /// from, so "an overlay row goes away" is one piece of code with two
    /// callers rather than a rule each verb remembers. Whether the draft it
    /// held is OVER is the callers' question, not this one's.
    async fn clear_row(&self, domain_id: DomainId, path: &str) -> Result<()> {
        let actor = self.writing_actor()?;
        let domain = self.domain.as_str();
        let state_dir = self.engine.journal_state_dir()?;
        crate::overlay_journal::journal_clear(&state_dir, domain, actor, path).map_err(
            |source| EngineError::Io {
                path: state_dir.display().to_string(),
                source,
            },
        )?;
        let store = self.engine.store();
        let store = store.lock().await;
        store.begin().await?;
        let done = async {
            store.clear_overlay_entry(domain_id, actor, path).await?;
            // In the same transaction, because a row that is gone and an edge
            // that still names it are one fact told two ways: this author's
            // other drafts that pointed here fall back onto whatever they read
            // at that address now - the base row where one stands, nothing
            // where none does. The fold, the discard and a settled convergence
            // all end a draft through here, so all three get it.
            store.reresolve_actor_references(domain_id, actor).await?;
            Ok::<(), EngineError>(())
        }
        .await;
        match done {
            Ok(()) => {
                store.commit().await?;
                Ok(())
            }
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }

    /// [`Engine::resolve_in`] for a call that may be acting inside a draft
    /// overlay: what THIS actor sees at that identifier.
    ///
    /// The rule itself is [`DomainView::shadow`]'s, which is also what the read
    /// path applies - see there for why the two must be one function.
    pub(crate) async fn resolve(
        &self,
        identifier: &str,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        let domain = self.domain.as_str();
        let base = self.engine.resolve_in(identifier, domain).await;
        self.shadow(identifier, base, || {
            format!("no engram '{identifier}' in domain '{domain}'")
        })
        .await
    }

    /// The actor dimension applied to a base resolution: **the one place the
    /// rule lives**, so a read and a write at one address cannot disagree
    /// about what it names.
    ///
    /// They did disagree. The read path was taught that a tombstone is about a
    /// PATH rather than about an address, and the write path was not, so an
    /// author who had moved her own draft of a page the team holds could open
    /// it at its address and not save it there - one screen, one URL, two
    /// answers. The repair is not the same three lines in two places, which is
    /// how the split happened; it is one function with two callers.
    ///
    /// The rule, in the order it is asked:
    ///
    /// * **the base row, unless this actor deleted that path.** A draft over a
    ///   base row resolves to the BASE descriptor: its path, domain and
    ///   permalink are what a write needs, the draft's own row is read by the
    ///   arm that writes it, and resolving to the base is what keeps one engram
    ///   one address whether or not this actor has started drafting it;
    /// * **behind their own tombstone, their own drafts.** A tombstone says
    ///   this PATH holds nothing for them, and an address can move off a path:
    ///   renaming a draft of a page the team holds leaves a tombstone where the
    ///   base row is and their own row, carrying the same address, somewhere
    ///   else, because a document travels verbatim. Answering the miss here
    ///   would lose the address for its own author while everybody else went on
    ///   reading the page;
    /// * **the miss otherwise** - a plain deletion is a deletion for the person
    ///   who made it, in the same words an engram nobody wrote produces, and
    ///   `tombstoned` is what says it. A base that knew nothing at all keeps its
    ///   OWN miss, so an identifier nobody wrote reads the same in both modes;
    /// * **and a draft at a path no base row holds** resolves through the
    ///   draft's own row, which is the only way an engram created in review
    ///   mode can be edited, moved or deleted at all.
    ///
    /// The base view is handed its resolution back without a single lookup, so
    /// a reader with no drafts resolves exactly what they always did.
    ///
    /// `tombstoned` is called only on the one arm that needs it, because the
    /// two callers word that miss differently: a bare identifier does not name
    /// a domain, so the answer it gets must not either.
    pub(crate) async fn shadow(
        &self,
        identifier: &str,
        base: Result<(EngramDescriptor, ContentSource)>,
        tombstoned: impl FnOnce() -> String,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        if self.actor.is_none() {
            return base;
        }
        match base {
            Ok((desc, source)) => {
                if self.deletes(desc.domain_id, &desc.path).await? {
                    return match self.resolve_draft(identifier).await? {
                        Some(found) => Ok(found),
                        None => Err(EngineError::NotFound(tombstoned())),
                    };
                }
                Ok((desc, source))
            }
            Err(EngineError::NotFound(miss)) => match self.resolve_draft(identifier).await? {
                Some(found) => Ok(found),
                None => Err(EngineError::NotFound(miss)),
            },
            Err(e) => Err(e),
        }
    }

    /// One actor's own draft at an identifier, when no base row answers to it.
    ///
    /// Matched by permalink, by title and by path, which is the same ladder
    /// the base lookup offers, over the entries this actor holds. Tombstones
    /// are skipped: a deletion is not an engram to find.
    pub(crate) async fn resolve_draft(
        &self,
        identifier: &str,
    ) -> Result<Option<(EngramDescriptor, ContentSource)>> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return Ok(None);
        };
        // An absolute identifier naming another domain is not this domain's to
        // answer, exactly as `resolve_in` refuses it.
        let wanted = match CrystallineUrl::parse(identifier) {
            Some(url) if url.domain != domain => return Ok(None),
            Some(url) => url.permalink,
            None => identifier.to_string(),
        };
        let (domain_id, source) = self.engine.domain_source(domain).await?;
        let entries = {
            let store = self.engine.store();
            let store = store.lock().await;
            store.overlay_entries(domain_id, actor).await?
        };
        for entry in entries {
            if entry.tombstone {
                continue;
            }
            let Ok(engram) = parse_engram(&entry.content) else {
                continue;
            };
            let record =
                EngramRecord::from_engram(&engram, &entry.path, virtual_stamp(&entry.content));
            let names = [
                entry.permalink.as_str(),
                record.title.as_str(),
                entry.path.as_str(),
            ];
            if !names.iter().any(|name| *name == wanted) {
                continue;
            }
            return Ok(Some((
                EngramDescriptor {
                    id: entry.id,
                    domain_id,
                    domain: domain.to_string(),
                    path: entry.path,
                    permalink: entry.permalink,
                    title: record.title,
                    engram_type: record.engram_type,
                    status: record.status,
                },
                source,
            )));
        }
        Ok(None)
    }

    /// The seeds of a graph traversal, in one reader's own view of the domain.
    ///
    /// Three edits, the same three [`Engine::shadow_level`] makes to a browse
    /// level: a path this reader has deleted seeds nothing, a path they are
    /// drafting seeds from their own row - so the traversal walks the edges
    /// they wrote rather than the ones the reviewed file carries - and a draft
    /// at a path no file holds is a seed like any other. `None` hands the base
    /// seeds back untouched, which is what a reader with no identity gets and
    /// what every caller got before the dimension existed.
    ///
    /// The registered-set screen composes ahead of this, never behind it: a
    /// caller passes `None` for a domain the reader may not see, so a draft of
    /// their own is no way back into a domain that is hidden from them.
    pub(crate) async fn list_over(
        &self,
        base: Vec<EngramDescriptor>,
    ) -> Result<Vec<EngramDescriptor>> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return Ok(base);
        };
        let (domain_id, _) = self.engine.domain_source(domain).await?;
        let entries = {
            let store = self.engine.store();
            let store = store.lock().await;
            store.overlay_entries(domain_id, actor).await?
        };
        let held: HashMap<&str, &crystalline_index::StoredEngram> = entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let mut seeds: Vec<EngramDescriptor> = Vec::with_capacity(base.len());
        let mut base_paths: HashSet<String> = HashSet::new();
        for d in base {
            base_paths.insert(d.path.clone());
            match held.get(d.path.as_str()) {
                // Deleted: this reader anchors on nothing here, and nothing
                // reaches them through it either.
                Some(entry) if entry.tombstone => {}
                Some(entry) => seeds.extend(overlay_descriptor(domain, domain_id, entry)),
                None => seeds.push(d),
            }
        }
        for entry in &entries {
            if !base_paths.contains(&entry.path) {
                seeds.extend(overlay_descriptor(domain, domain_id, entry));
            }
        }
        Ok(seeds)
    }

    /// [`Engine::shadow_seeds`] for a single named anchor: the base row this
    /// reader sees at that address, their own draft of it when they hold one,
    /// nothing at all when they have deleted it, and their draft-only engram
    /// when no file holds that address at all.
    pub(crate) async fn anchor(
        &self,
        base: Option<EngramDescriptor>,
        permalink: &str,
    ) -> Result<Option<EngramDescriptor>> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return Ok(base);
        };
        match base {
            // One path, so one targeted lookup rather than a scan of this
            // actor's whole overlay: the question is only ever "does this
            // reader hold a row at the path the base row stands at".
            Some(d) => {
                let (domain_id, _) = self.engine.domain_source(domain).await?;
                let entry = {
                    let store = self.engine.store();
                    let store = store.lock().await;
                    store.overlay_entry(domain_id, actor, &d.path).await?
                };
                Ok(match entry {
                    Some(entry) if entry.tombstone => None,
                    Some(entry) => overlay_descriptor(domain, domain_id, &entry),
                    None => Some(d),
                })
            }
            None => Ok(self.resolve_draft(permalink).await?.map(|(desc, _)| desc)),
        }
    }

    /// The row whose outbound edges a reader sees for an engram: their own
    /// draft's row when they hold one at that path, the base row otherwise.
    ///
    /// A draft-only engram already carries its own id, because nothing else
    /// could have described it; this is about a draft that stands over a base
    /// row, whose descriptor is the base's so that one engram keeps one
    /// address. Its edges are not the base's.
    pub(crate) async fn edge_id(&self, desc: &EngramDescriptor) -> Result<EngramId> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(desc.id);
        };
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store
            .overlay_entry(desc.domain_id, actor, &desc.path)
            .await?
            .filter(|entry| !entry.tombstone)
            .map(|entry| entry.id)
            .unwrap_or(desc.id))
    }

    /// The address one actor's own draft at `path` answers to, or `None` when
    /// they hold no draft there (or hold a deletion, which answers to no
    /// address at all).
    pub(crate) async fn draft_permalink_at(&self, path: Option<&str>) -> Result<Option<String>> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(None);
        };
        let Some(path) = path else {
            return Ok(None);
        };
        let (domain_id, _) = self.engine.domain_source(&self.domain).await?;
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store
            .overlay_entry(domain_id, actor, path)
            .await?
            .filter(|entry| !entry.tombstone)
            .map(|entry| entry.permalink))
    }

    /// A move inside one actor's draft overlay: a tombstone at the source and
    /// an entry at the destination.
    ///
    /// The shape a rename in review mode has to take, and the shape a later
    /// convergence pass has to recognize: the reviewed file stays exactly where
    /// the team put it, and this actor's view of the domain has the engram at
    /// its new address until the move is reviewed.
    ///
    /// A cross-domain move is refused rather than half-performed. A draft
    /// belongs to the domain it is drafted in - it has no row, no file and no
    /// reviewer anywhere else - so carrying one across would either write into
    /// a domain that never reviewed it or leave the engram in two places at
    /// once, and the refusal names the order that works instead.
    ///
    /// `new_permalink` is the permalink the caller asked for, `None` when they
    /// named none. Unnamed, the document travels verbatim and keeps the
    /// address it carries, which is how a later fold recognizes the draft as
    /// a move of the team's engram. Named, it is written into the draft's
    /// `permalink:` line when the text would not answer to it on its own, with
    /// `mover` in its `generated` block, exactly as a direct move writes it. A
    /// destination equal to the source path is the permalink-only rename, and
    /// in a draft that is one write rather than two: the draft at that path
    /// with its new `permalink:` line. References in other engrams are not
    /// rewritten here - each of those would be a draft of its own, of an
    /// engram the mover never opened - so they follow when the move is shared
    /// and moved by the team, and `links_rewritten` stays 0.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn move_within(
        &self,
        p: &MoveParams,
        src: &EngramDescriptor,
        src_source: &ContentSource,
        dest_rel: &str,
        cross: bool,
        new_permalink: Option<&str>,
        mover: &str,
    ) -> Result<Value> {
        let actor = self.writing_actor()?;
        if cross {
            return Err(EngineError::Refused(format!(
                "'{}' reviews changes before they land, so this engram is a draft, and a draft \
                 moves only inside the domain it is drafted in - share the change first, then \
                 move the engram the team has",
                p.domain
            )));
        }
        let in_place = dest_rel == src.path;
        if in_place && new_permalink.is_none_or(|asked| asked == src.permalink) {
            return Err(EngineError::Invalid(
                "the destination is where the engram already is".into(),
            ));
        }
        // Free in THIS actor's view, which is the only view the move happens
        // in: a path another actor is drafting at is not taken for this one,
        // and a base row at the destination is, since the moved engram would
        // shadow it rather than land beside it. An in-place rename is its own
        // destination.
        if !in_place {
            let store = self.engine.store();
            let store = store.lock().await;
            let taken = store
                .overlay_entry(src.domain_id, actor, dest_rel)
                .await?
                .map(|entry| !entry.tombstone)
                .unwrap_or(false)
                || store
                    .list_engrams(&p.domain, Some(dest_rel), None)
                    .await?
                    .iter()
                    .any(|found| found.path == dest_rel);
            if taken {
                return Err(EngineError::Conflict(format!(
                    "'{dest_rel}' already holds an engram in domain '{}'",
                    p.domain
                )));
            }
        }
        let text = self.text_at(src_source, src).await?.ok_or_else(|| {
            EngineError::NotFound(format!(
                "no engram '{}' in domain '{}'",
                p.identifier, p.domain
            ))
        })?;
        // Where the engram would answer from once it has moved: the document
        // travels verbatim, so the address travels with it unless the
        // frontmatter never carried one and the path's own slug is it - and
        // when the caller named another, the draft's `permalink:` line is
        // rewritten to say so.
        let implied = parse_engram(&text)
            .map(|engram| {
                EngramRecord::from_engram(&engram, dest_rel, virtual_stamp(&text)).permalink
            })
            .unwrap_or_else(|_| src.permalink.clone());
        let (text, dest_permalink) = match new_permalink {
            Some(asked) if asked != implied => (
                touch_generated(
                    &set_frontmatter_field(&text, "permalink", asked),
                    mover,
                    None,
                    chrono::Utc::now().fixed_offset(),
                ),
                asked.to_string(),
            ),
            _ => (text, implied),
        };
        // Asked BEFORE either write, although the writer below asks it again:
        // a move is two writes, and a refusal that arrived at the second one
        // would already have tombstoned or dropped the source, leaving the
        // rollback to put back a draft that lives in that row and nowhere
        // else. The source does not count against itself - it is about to be a
        // tombstone, which answers to no address, or gone.
        self.refuse_address_held_elsewhere(
            src.domain_id,
            &dest_permalink,
            dest_rel,
            Some(&src.path),
        )
        .await?;
        // The permalink-only rename: one write, the draft at the path it
        // already has, answering to its new name. Nothing is tombstoned and no
        // grant ends, since a grant was minted on the path and the path stays.
        if in_place {
            let warning = self.write(src.domain_id, &src.path, &text).await?;
            let mut receipt = json!({
                "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
                "to": { "domain": p.domain, "permalink": dest_permalink, "path": src.path },
                "cross_domain": false,
                "links_rewritten": 0,
                "references_rewritten": 0,
                "rewritten": Vec::<Value>::new(),
                "attachment_warnings": Vec::<String>::new(),
                "draft": true,
            });
            note_unmirrored(&mut receipt, warning);
            return Ok(receipt);
        }
        // The SOURCE first, and the order is forced rather than preferred: one
        // actor holds one row per permalink per domain, and until the source is
        // a tombstone (which answers to no permalink) or gone, the engram's own
        // permalink is still spoken for and the destination cannot take it.
        //
        // Which is why the destination's failure puts the source back. Once the
        // source is a tombstone this actor reads nothing at that path, so a
        // retry would not find the engram to move and the text - which for a
        // draft lives in that row and nowhere else - would be gone. The
        // rollback is what makes a move that fails a move that did not happen.
        let held_draft = {
            let store = self.engine.store();
            let store = store.lock().await;
            store
                .overlay_entry(src.domain_id, actor, &src.path)
                .await?
                .is_some_and(|entry| !entry.tombstone)
        };
        let base = {
            let store = self.engine.store();
            let store = store.lock().await;
            store
                .list_engrams(&p.domain, Some(&src.path), None)
                .await?
                .into_iter()
                .find(|found| found.path == src.path)
        };
        let mut warnings: Vec<String> = Vec::new();
        match &base {
            // A draft of a path no file holds was only ever this actor's, so
            // the move takes it with them; a tombstone over nothing would
            // leave a deletion of an engram the team never had.
            None => {
                // Deferred, like every other half-done step of this verb: the
                // ending is thirty lines below, after the destination lands.
                self.drop_mid_move(src.domain_id, &src.path).await?;
            }
            Some(base) => {
                let base_text = self.engine.load_content(src_source, base).await?;
                warnings.extend(
                    self.engine
                        .write_overlay_tombstone(&p.domain, actor, base, &base_text)
                        .await?,
                );
            }
        }
        match self.write(src.domain_id, dest_rel, &text).await {
            Ok(warning) => warnings.extend(warning),
            Err(e) => {
                // The destination's ROW did not land - a mirror that failed
                // would have come back as a warning above - so there is nothing
                // at the destination to collide with, and the source goes back
                // to exactly what this actor held: their own draft when they
                // had one, and otherwise nothing of their own at all, which is
                // the base row showing through again.
                let undo = if held_draft {
                    self.write_unchecked(src.domain_id, &src.path, &text)
                        .await
                        .map(|_| ())
                } else {
                    self.drop_mid_move(src.domain_id, &src.path).await
                };
                if let Err(undo) = undo {
                    tracing::error!(
                        domain = p.domain.as_str(),
                        path = src.path.as_str(),
                        "the move could not write the destination and could not put the source \
                         back either: {undo}"
                    );
                }
                return Err(e);
            }
        }
        // The draft has left the path it was shared at, so every link on that
        // path and every session inside it ends with it - the same call the
        // discard paths make, for the same reason: a grant lasts exactly as
        // long as the thing it grants, and a row left live would spring back
        // onto whatever its author drafted at the old path next.
        //
        // **After the destination write, never before.** A move that could not
        // write the destination puts the source back and did not happen, and a
        // move that did not happen must not have ended anybody's link on the
        // way to not happening.
        //
        // The grant does not follow the rename, deliberately: it was minted on
        // a path, the author is the one who knows whether the page is still the
        // page they shared, and re-sharing it under its new name is one press.
        self.engine
            .end_draft_grants(&p.domain, actor, &src.path)
            .await;
        let mut receipt = json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": p.domain, "permalink": dest_permalink, "path": dest_rel },
            "cross_domain": false,
            "links_rewritten": 0,
            "references_rewritten": 0,
            "rewritten": Vec::<Value>::new(),
            "attachment_warnings": Vec::<String>::new(),
            "draft": true,
        });
        // A move is two writes and either mirror can fail on its own, so the
        // receipt carries whichever of them did rather than the first.
        note_unmirrored(
            &mut receipt,
            (!warnings.is_empty()).then(|| warnings.join(" ")),
        );
        Ok(receipt)
    }

    /// Fold one reader's drafts into a recency listing.
    ///
    /// The same three edits shadowing means everywhere: a row this actor has
    /// tombstoned leaves, a row they are drafting is described by their draft,
    /// and a draft the base listing does not hold joins it. The result is
    /// re-sorted and re-cut exactly as the statement sorted and cut it, so a
    /// caller reading `count` reads the count of what came back.
    ///
    /// **The base page was already cut to the limit in SQL**, so a draft
    /// joining it can push out a base row that would otherwise have been the
    /// last one shown, and a tombstone over a row below the cut subtracts
    /// nothing. Both are the same imprecision [`Engine::shadow_level`] carries
    /// and for the same reason: an exact answer needs the actor threaded into
    /// the statement, which is the index-side work Task 5 does for search.
    /// Neither can hide a draft from its own author, which is the hole this
    /// closes.
    pub(crate) async fn recent_into(
        &self,
        filter: &RecentFilter,
        items: &mut Vec<EngramSummary>,
    ) -> Result<()> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return Ok(());
        };
        {
            let (domain_id, _) = self.engine.domain_source(domain).await?;
            let entries = {
                let store = self.engine.store();
                let store = store.lock().await;
                store.overlay_entries(domain_id, actor).await?
            };
            if entries.is_empty() {
                return Ok(());
            }
            for entry in &entries {
                // What the base row at this path answers to, which is the only
                // thing that ties a draft to the row it shadows here: a
                // recency listing carries no path.
                let shadowed = {
                    let store = self.engine.store();
                    let store = store.lock().await;
                    store
                        .list_engrams(domain, Some(&entry.path), None)
                        .await?
                        .into_iter()
                        .find(|found| found.path == entry.path)
                        .map(|found| found.permalink)
                };
                items.retain(|item| {
                    item.domain != domain
                        || (Some(&item.permalink) != shadowed.as_ref()
                            && item.permalink != entry.permalink)
                });
                if entry.tombstone {
                    continue;
                }
                let Ok(engram) = parse_engram(&entry.content) else {
                    continue;
                };
                let record =
                    EngramRecord::from_engram(&engram, &entry.path, virtual_stamp(&entry.content));
                // The caller's own filters, applied to a draft exactly as the
                // statement applied them to a base row.
                let recorded = record.recorded_at.map(|at| at.to_string());
                if filter
                    .after
                    .as_ref()
                    .is_some_and(|after| recorded.as_ref().is_none_or(|at| at < after))
                {
                    continue;
                }
                if filter
                    .engram_types
                    .as_ref()
                    .is_some_and(|types| !types.contains(&record.engram_type))
                {
                    continue;
                }
                items.push(EngramSummary {
                    domain: domain.to_string(),
                    permalink: entry.permalink.clone(),
                    title: record.title,
                    engram_type: record.engram_type,
                    status: record.status,
                    recorded_at: recorded,
                    tags: record.tags,
                });
            }
        }
        Ok(())
    }

    /// Fold one reader's drafts into a browse level.
    ///
    /// Three edits, which are the whole of what shadowing means for a listing:
    /// a row this actor has tombstoned leaves, a row they are drafting is
    /// described by their draft rather than by the file, and a draft at a path
    /// the domain's files never held joins the level and contributes its folder
    /// if it is in one.
    ///
    /// **The level's `total` is adjusted by what this pass can see, and on a
    /// level the cap already cut that is the base level's count plus this
    /// actor's drafts rather than an exact one.** `browse_level` pushes the
    /// prefix, the depth and [`TREE_LEVEL_CAP`] into SQL and counts under the
    /// same filter, so a tombstone over a row that fell outside the returned
    /// page cannot be subtracted here without reading the page the cap
    /// withheld. A cut level is already telling its client to ask for the
    /// listing instead; an exact count for one needs the actor threaded into
    /// the statement, which is the index-side work Task 5 does for search.
    pub(crate) async fn level(
        &self,
        prefix: Option<&str>,
        depth: usize,
        level: &mut BrowseLevel,
    ) -> Result<()> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return Ok(());
        };
        let (domain_id, _) = self.engine.domain_source(domain).await?;
        let entries = {
            let store = self.engine.store();
            let store = store.lock().await;
            store.overlay_entries(domain_id, actor).await?
        };
        if entries.is_empty() {
            return Ok(());
        }
        let folder = folder_slash_lower(prefix.unwrap_or_default());
        let mut drafts: HashMap<String, &crystalline_index::StoredEngram> = HashMap::new();
        let mut tombstones: HashSet<String> = HashSet::new();
        for entry in &entries {
            if entry.tombstone {
                tombstones.insert(entry.path.clone());
            } else {
                drafts.insert(entry.path.clone(), entry);
            }
        }

        let before = level.engrams.len();
        level.engrams.retain(|d| !tombstones.contains(&d.path));
        let removed = before - level.engrams.len();

        let mut seen: HashSet<String> = level.engrams.iter().map(|d| d.path.clone()).collect();
        for row in level.engrams.iter_mut() {
            if let Some(draft) = drafts.get(&row.path) {
                overwrite_from_draft(row, draft);
            }
        }

        // The drafts the base level does not hold, at this level and under this
        // prefix: the same two cuts `browse_level` makes in SQL, made here in
        // Rust over a handful of rows.
        let mut added = 0usize;
        let mut folders: BTreeSet<String> = level.folders.iter().cloned().collect();
        for entry in &entries {
            if entry.tombstone || seen.contains(&entry.path) {
                continue;
            }
            let lowered = entry.path.to_lowercase();
            let Some(rel) = lowered.strip_prefix(folder.as_str()) else {
                continue;
            };
            let rel = &entry.path[entry.path.len() - rel.len()..];
            if let Some((head, _)) = rel.split_once('/') {
                folders.insert(head.to_string());
            }
            if rel.matches('/').count() >= depth {
                continue;
            }
            let mut row = EngramDescriptor {
                id: entry.id,
                domain_id,
                domain: domain.to_string(),
                path: entry.path.clone(),
                permalink: entry.permalink.clone(),
                title: String::new(),
                engram_type: String::new(),
                status: String::new(),
            };
            overwrite_from_draft(&mut row, entry);
            seen.insert(entry.path.clone());
            level.engrams.push(row);
            added += 1;
        }
        level.engrams.sort_by(|a, b| a.path.cmp(&b.path));
        level.folders = folders.into_iter().collect();
        level.total = level.total.saturating_sub(removed) + added;
        Ok(())
    }

    /// This actor's overlay rows over the domain, read in a single query,
    /// tombstones included - which is exactly where their view and the domain's
    /// own rows disagree.
    ///
    /// The base view - a domain that takes changes directly, or a caller with
    /// no identity of their own - answers with no rows at all, which is what
    /// makes every reader of it degrade to the base behaviour without a branch
    /// of their own.
    pub(crate) async fn entries(&self, domain_id: DomainId) -> Result<Vec<StoredEngram>> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(Vec::new());
        };
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store.overlay_entries(domain_id, actor).await?)
    }

    /// The document this actor holds at each path they are drafting at, which
    /// is [`DomainView::entries`] with the deletions left out: a tombstone is
    /// not an engram to assemble anything from.
    pub(crate) fn held_text(entries: &[StoredEngram]) -> HashMap<String, String> {
        entries
            .iter()
            .filter(|entry| !entry.tombstone)
            .map(|entry| (entry.path.clone(), entry.content.clone()))
            .collect()
    }

    /// The attachment paths the domain's OWN text references or claims at the
    /// paths one actor's view has replaced or removed.
    ///
    /// `V108`'s other half. An attachment is shared state and deleting one is a
    /// shared act, so the question "does anything reference this file" is asked
    /// of the union rather than of one reader: an author who drafts a reference
    /// away is never told the file is now unused, and neither is anybody else.
    /// The base document is what is read here - [`Engine::load_engram`] takes
    /// the file for a file domain and the `actor = ''` row for a virtual one -
    /// so a path only this actor's overlay holds contributes nothing, having no
    /// shared text to speak for it.
    ///
    /// Best effort by construction: a base document that no longer parses is
    /// skipped, which is the same answer the fact assembly gives it.
    pub(crate) async fn shadowed_asset_refs(
        &self,
        domain_id: DomainId,
        entries: &[StoredEngram],
    ) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for entry in entries {
            let Some(engram) = self.base_engram_at(domain_id, &entry.path).await else {
                continue;
            };
            out.extend(crystalline_core::find_asset_refs(&engram.body));
            out.extend(asset_claim(&engram.frontmatter));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Every reference leaving one engram, judged as this view judges it.
    ///
    /// Two questions in one call and they are both the view's. WHOSE row the
    /// references come off is [`DomainView::edge_id`]: a draft over a base row
    /// resolves to the base descriptor, so one engram keeps one address, but
    /// the relations and prose links in front of the reader are the ones their
    /// own document wrote. And whether each one LANDS is asked in their view
    /// too, which moves exactly two verdicts: a reference into a path they
    /// have deleted dangles for them, and a dangling one their own draft
    /// answers lands for them.
    ///
    /// Inbound stays the base row's and is deliberately not here: who points
    /// at an address is a fact about the address the team shares, and nobody
    /// can write a reference to a draft only its author can read.
    pub(crate) async fn outbound(&self, desc: &EngramDescriptor) -> Result<Vec<OutboundRef>> {
        let edge_id = self.edge_id(desc).await?;
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store.outbound_refs(edge_id, self.actor()).await?)
    }

    /// The dangling references in this view of a domain, as the sweep's `V102`
    /// input.
    ///
    /// One query, one answer, and both halves of the reader's view are in it:
    /// the base rows their drafts do not shadow, judged against what they hold
    /// at each target's address, plus their own drafts' references judged the
    /// same way. `None` is the domain's own queue, byte for byte what it was.
    ///
    /// This replaced a pass that re-implemented the resolution forms in Rust
    /// over the shadowed listing. It could not see the colon-prefixed title
    /// form at all, and it could not be asked about a base row - so a base
    /// link one reader's draft had already answered was still raised at them.
    /// Both are the same bug: a second answer to "does this link resolve".
    pub(crate) async fn unresolved(
        &self,
        domain_id: DomainId,
    ) -> Result<Vec<crystalline_index::UnresolvedRef>> {
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store.unresolved_refs(domain_id, self.actor()).await?)
    }

    /// The vocabulary in use in this view of the domain.
    ///
    /// Base by default and by design: every surface that shows a PERSON the
    /// vocabulary asks a base view, because a tag one author is trying out in
    /// a draft is not yet the domain's agreement and the team's own clusters
    /// are what an author should be reading either way. The one caller that
    /// holds an actor view here is the sweep, whose tag-drift finding is about
    /// what that author wrote.
    pub(crate) async fn vocabulary(&self) -> Result<crystalline_index::Vocabulary> {
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store.vocabulary(Some(&self.domain), self.actor()).await?)
    }

    /// This actor's own live draft at a path, or `None` on the base view, at a
    /// path they hold nothing at, and at one they have deleted.
    ///
    /// The marker a read verb says "this is your draft" with. A deletion
    /// answers `None` because there is no draft to be reading there: the verb
    /// that meets one answers not-found, through [`DomainView::deletes`].
    pub(crate) async fn draft_at(
        &self,
        domain_id: DomainId,
        path: &str,
    ) -> Result<Option<StoredEngram>> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(None);
        };
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store
            .overlay_entry(domain_id, actor, path)
            .await?
            .filter(|entry| !entry.tombstone))
    }

    /// Whether this reader holds ANY row of their own at `path`, a deletion
    /// included.
    ///
    /// [`DomainView::draft_at`] beside it filters tombstones out, because it
    /// answers "is there a draft to show". This one answers a different
    /// question - "has this reader made this path their own" - and a deletion
    /// is as much an answer to that as a redraft is. The one caller is
    /// [`crate::engine::Engine::granted_read`], which must not put somebody
    /// else's draft where a reader has put their own decision, whichever
    /// decision it was.
    ///
    /// Reads this view's OWN actor and nobody else's, which is what makes it a
    /// question about the caller rather than a second seam onto another
    /// reader's rows.
    pub(crate) async fn holds_own_entry(&self, domain_id: DomainId, path: &str) -> Result<bool> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(false);
        };
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store.overlay_entry(domain_id, actor, path).await?.is_some())
    }

    /// One engram's exact text and identity, as this view sees it: what the
    /// collab session layer loads at open and probes with on its idle
    /// external-change check.
    ///
    /// Deliberately thin - [`Engine::read_engram`] resolves references and
    /// builds hints this caller never reads. **Whose text** is the view's to
    /// say, and every caller today builds a [`DomainView::base`], so a room in
    /// a domain that reviews changes opens on the text the team reviewed,
    /// whoever else is drafting.
    pub(crate) async fn engram_text(&self, identifier: &str) -> Result<EngramText> {
        let (desc, source) = self.resolve(identifier).await?;
        let content = self.text_at(&source, &desc).await?.ok_or_else(|| {
            EngineError::NotFound(format!(
                "no engram '{identifier}' in domain '{}'",
                self.domain
            ))
        })?;
        let checksum = sha256_hex(content.as_bytes());
        Ok(EngramText {
            domain: desc.domain,
            permalink: desc.permalink,
            path: desc.path,
            content,
            checksum,
        })
    }

    /// The exact text this view holds at a domain-relative PATH right now, or
    /// `None` when nothing is there.
    ///
    /// The base substrate, on the base view every caller builds today: a collab
    /// room whose engram vanished from the index probes the path before it puts
    /// its own text back, and what it must not overwrite is what the team's
    /// folder holds. See [`Engine::engram_text_at_path`], the name-addressed
    /// form of this.
    pub(crate) async fn engram_text_at_path(&self, path: &str) -> Result<Option<EngramText>> {
        let domain = self.domain.as_str();
        let source = self.engine.content_source(domain)?;
        let content = match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, path);
                match std::fs::read_to_string(&abs) {
                    Ok(text) => text,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                }
            }
            ContentSource::Virtual => {
                let (domain_id, _) = self.engine.domain_source(domain).await?;
                let store = self.engine.store();
                let store = store.lock().await;
                match store.engram_content(domain_id, path).await? {
                    Some(text) => text,
                    None => return Ok(None),
                }
            }
        };
        let permalink = {
            let store = self.engine.store();
            let store = store.lock().await;
            store
                .list_engrams(domain, Some(path), None)
                .await?
                .into_iter()
                .find(|found| found.path == path)
                .map(|found| found.permalink)
                .unwrap_or_else(|| path.trim_end_matches(".md").to_string())
        };
        Ok(Some(EngramText {
            domain: domain.to_string(),
            permalink,
            path: path.to_string(),
            checksum: sha256_hex(content.as_bytes()),
            content,
        }))
    }

    /// Whether this reader has deleted the path: `true` only when their own
    /// tombstone stands there.
    ///
    /// The whole of what a resolver has to ask once the base row is already in
    /// hand - a deletion is a deletion for its author, however the engram was
    /// addressed - and named for what it answers rather than as an `exists`,
    /// which would read as a question about the path and is not one: the base
    /// view deletes nothing and says so without a lookup.
    pub(crate) async fn deletes(&self, domain_id: DomainId, path: &str) -> Result<bool> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(false);
        };
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(store
            .overlay_entry(domain_id, actor, path)
            .await?
            .is_some_and(|entry| entry.tombstone))
    }

    /// The domain's OWN document at a path, read from the base substrate: the
    /// file for a file domain and the `actor = ''` row for a virtual one.
    ///
    /// Spelled as a base-substrate read on a type whose other operations answer
    /// for a reader, because that mixedness is deliberate and was unnamed
    /// before: `V108` asks "does anything reference this file" of the union
    /// rather than of one reader, so the paths come off this view and the text
    /// comes off the base.
    pub(crate) async fn base_engram_at(
        &self,
        domain_id: DomainId,
        path: &str,
    ) -> Option<crystalline_core::Engram> {
        let source = self.substrate()?;
        self.engine.load_engram(source, domain_id, path).await
    }

    /// The state directory this view's files overlay lives under, resolved
    /// through the engine's one resolver so the `testing` refusal that guards
    /// the journal root guards this root the same way.
    fn files_state_dir(&self) -> Result<std::path::PathBuf> {
        self.engine.journal_state_dir()
    }

    /// An error from the files overlay, named by the path it was about.
    ///
    /// A failure here is a failure, unlike a journal mirror's: the journal is a
    /// copy of a row that already landed, and this tree is the only place a
    /// draft file's bytes exist at all.
    fn files_io(&self, path: &str, source: std::io::Error) -> EngineError {
        EngineError::Io {
            path: format!("the files overlay of '{}' at '{path}'", self.domain),
            source,
        }
    }

    /// Every attachment this reader sees in the domain, metadata only on the
    /// base rows.
    ///
    /// The base view answers the folder's own rows and nothing else. An actor
    /// view answers what that actor sees: a path they have deleted is absent, a
    /// path they have written stands with their own bytes described, and a file
    /// only they hold is a row like any other. Ordered by path either way, so a
    /// listing reads the same whoever asked.
    ///
    /// **An actor's own overlay files are read and hashed here**, where the
    /// base listing is one query and no bytes. `N` is one actor's own draft
    /// files rather than the domain's, which is what makes that affordable -
    /// and it is what makes `sha256`, `size` and `modified` on an overlay row
    /// mean exactly what they mean on a base row, so a client caching on the
    /// checksum is not lied to about bytes only it can see.
    #[doc(hidden)]
    pub async fn attachments(&self) -> Result<Vec<AttachmentRow>> {
        let base = self.engine.attachment_list(&self.domain).await?;
        let Some(actor) = self.actor.as_deref() else {
            return Ok(base);
        };
        let held = self.files()?;
        if held.unreadable {
            // A listing short by an unknown number is still the best answer
            // there is - the alternative is refusing a read because one file
            // could not be enumerated - but it is never passed off as complete.
            tracing::warn!(
                domain = self.domain.as_str(),
                actor = actor,
                "part of this actor's files overlay could not be read; their attachment listing \
                 is short by what could not be reached"
            );
        }
        let held = held.entries;
        if held.is_empty() {
            return Ok(base);
        }
        let state_dir = self.files_state_dir()?;
        let hidden: BTreeSet<&str> = held.iter().map(|entry| entry.path.as_str()).collect();
        let mut rows: Vec<AttachmentRow> = base
            .into_iter()
            .filter(|row| !hidden.contains(row.path.as_str()))
            .collect();
        for entry in held.iter().filter(|entry| !entry.tombstone) {
            rows.push(self.overlay_attachment_row(&state_dir, actor, &entry.path)?);
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(rows)
    }

    /// The row describing one of this actor's own overlay files: the bytes it
    /// holds and the file's own modification instant, through the same builder
    /// the folder's rows are built with.
    ///
    /// **This one does read the file**, because a row carries a checksum and a
    /// checksum is a fact about bytes. It is the listing's builder for that
    /// reason and nothing else's: a read hashes the bytes it is about to return
    /// ([`DomainView::own_attachment`]) and a size question is answered by a
    /// stat ([`DomainView::attachment_delete_size`]), so neither of them comes
    /// through here.
    fn overlay_attachment_row(
        &self,
        state_dir: &Path,
        actor: &str,
        path: &str,
    ) -> Result<AttachmentRow> {
        let bytes = crate::overlay_files::read(state_dir, &self.domain, actor, path)
            .map_err(|e| self.files_io(path, e))?
            .ok_or_else(|| {
                EngineError::NotFound(crate::engine::missing_attachment(&self.domain, path))
            })?;
        let abs = crate::overlay_files::file(state_dir, &self.domain, actor, path)
            .map_err(|e| self.files_io(path, e))?;
        crate::engine::attachment_row(path, &bytes, crate::engine::asset_modified(&abs))
    }

    /// One attachment's bytes and its metadata row, as this reader sees them.
    ///
    /// The three-way answer the whole files overlay is: this actor's own bytes
    /// when they hold some, [`EngineError::NotFound`] when they hold a deletion
    /// of the path, and the folder's own answer otherwise - row heal included,
    /// since that arm is the base's unchanged.
    #[doc(hidden)]
    pub async fn attachment_bytes(&self, path: &str) -> Result<(Vec<u8>, AttachmentRow)> {
        if let Some((bytes, row)) = self.own_attachment(path)? {
            return Ok((bytes, row));
        }
        self.engine.attachment_read(&self.domain, path).await
    }

    /// What deleting one attachment would take away, for a preview that must
    /// never be stricter than the act it previews.
    ///
    /// The same three-way answer [`DomainView::attachment_bytes`] gives, which
    /// is what keeps that rule true in review mode: a draft-only file the
    /// delete would remove has a size here rather than a miss, and a path this
    /// actor has already deleted is a miss here rather than the folder's size.
    pub(crate) async fn attachment_delete_size(&self, path: &str) -> Result<u64> {
        match self.own_held(path)? {
            Some(crate::overlay_files::Held::Bytes) => {
                let state_dir = self.files_state_dir()?;
                let actor = self.actor.as_deref().unwrap_or_default();
                // A stat, never a read - see [`crate::overlay_files::size`].
                // The path was there a moment ago and can be gone now, which is
                // the race the folder arm answers by falling through to the
                // recorded row; there is no row here, so a file that vanished
                // under the preview is a miss.
                crate::overlay_files::size(&state_dir, &self.domain, actor, path)
                    .map_err(|e| self.files_io(path, e))?
                    .ok_or_else(|| {
                        EngineError::NotFound(crate::engine::missing_attachment(&self.domain, path))
                    })
            }
            Some(crate::overlay_files::Held::Tombstone) => Err(EngineError::NotFound(
                crate::engine::missing_attachment(&self.domain, path),
            )),
            _ => self.engine.attachment_delete_size(&self.domain, path).await,
        }
    }

    /// What this actor holds at `path` in the files overlay, or [`None`] on the
    /// base view - which never reads the files overlay at all.
    ///
    /// The path is validated by the substrate on the way in, so an illegal one
    /// is refused here exactly as [`crate::engine::validate_attachment_path`]
    /// refuses it further down.
    fn own_held(&self, path: &str) -> Result<Option<crate::overlay_files::Held>> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(None);
        };
        let state_dir = self.files_state_dir()?;
        match crate::overlay_files::held(&state_dir, &self.domain, actor, path) {
            Ok(held) => Ok(Some(held)),
            // A path this substrate refuses is a path the attachment verbs
            // refuse; letting it through to the base arm is what keeps the one
            // refusal message the caller already knows.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                crate::engine::validate_attachment_path(path)?;
                Err(self.files_io(path, e))
            }
            Err(e) => Err(self.files_io(path, e)),
        }
    }

    /// This actor's own bytes and row at `path`: [`Some`] when they hold bytes,
    /// `NotFound` when they hold a deletion, [`None`] when the folder answers.
    fn own_attachment(&self, path: &str) -> Result<Option<(Vec<u8>, AttachmentRow)>> {
        match self.own_held(path)? {
            None | Some(crate::overlay_files::Held::Nothing) => Ok(None),
            Some(crate::overlay_files::Held::Tombstone) => Err(EngineError::NotFound(
                crate::engine::missing_attachment(&self.domain, path),
            )),
            Some(crate::overlay_files::Held::Bytes) => {
                let state_dir = self.files_state_dir()?;
                let actor = self.actor.as_deref().unwrap_or_default();
                // **One read, and the row describes what it returned.** A
                // strong `ETag` is built off `row.sha256` and promises the body
                // it rides with, so hashing a second read would let a replace
                // between the two hand this caller a validator for bytes they
                // never received. It is the hazard `put_file` already avoids on
                // the write side, and it is what the folder arm's own doc
                // promises when it says the sha describes exactly what was
                // received.
                let bytes = crate::overlay_files::read(&state_dir, &self.domain, actor, path)
                    .map_err(|e| self.files_io(path, e))?
                    .ok_or_else(|| {
                        EngineError::NotFound(crate::engine::missing_attachment(&self.domain, path))
                    })?;
                let abs = crate::overlay_files::file(&state_dir, &self.domain, actor, path)
                    .map_err(|e| self.files_io(path, e))?;
                let row = crate::engine::attachment_row(
                    path,
                    &bytes,
                    crate::engine::asset_modified(&abs),
                )?;
                Ok(Some((bytes, row)))
            }
        }
    }

    /// Write this actor's own copy of one non-engram path.
    ///
    /// **Refuses on the base view, and the refusal is checked before anything
    /// else.** A base view's write goes to the folder and is
    /// [`Engine::attachment_write`]'s, never this - so reaching here without an
    /// actor is a routing bug rather than a caller's mistake, and it says so in
    /// [`EngineError::Internal`] instead of teaching a caller something they
    /// cannot act on. Unreachable through a verb: the engine's view-taking
    /// write takes the base arm whenever the view has no actor.
    ///
    /// The row that comes back is built the way the folder's rows are built:
    /// off the bytes **the caller handed in** and the file's own modification
    /// instant, so nothing downstream can tell an overlay row from a base one
    /// by its shape. Hashing what is on disk instead would cost a second full
    /// read and would let a concurrent replace hand this caller a receipt
    /// describing the other writer's bytes - which is what the folder arm's
    /// per-file lock exists to prevent there.
    pub(crate) async fn put_file(&self, path: &str, bytes: &[u8]) -> Result<AttachmentRow> {
        let actor = self.files_writer()?;
        let state_dir = self.files_state_dir()?;
        crate::overlay_files::put(&state_dir, &self.domain, actor, path, bytes)
            .map_err(|e| self.files_io(path, e))?;
        let abs = crate::overlay_files::file(&state_dir, &self.domain, actor, path)
            .map_err(|e| self.files_io(path, e))?;
        crate::engine::attachment_row(path, bytes, crate::engine::asset_modified(&abs))
    }

    /// Mark this actor's deletion of one non-engram path.
    ///
    /// Two shapes, decided by whether the folder holds the path at all. A
    /// **reviewed** file is hidden behind a marker: the file stays where the
    /// team put it and reads absent for this actor alone, until the deletion is
    /// reviewed like any other change. A file **only this actor ever held**
    /// simply goes, marker and all - there is nothing to hide, and a marker
    /// standing over a base that was never there is exactly what convergence
    /// would clear again. It is the same rule a draft-only engram's delete
    /// follows when it drops the row rather than tombstoning it.
    ///
    /// "The folder holds it" is asked the way the delete itself asks it -
    /// either half, the file or the row - so a hand-edited domain's
    /// half-present pair is hidden rather than half-hidden.
    ///
    /// [`EngineError::NotFound`] when neither the overlay nor the folder holds
    /// the path, which is the miss [`Engine::attachment_delete`] reports.
    pub(crate) async fn tombstone_file(&self, path: &str) -> Result<()> {
        let actor = self.files_writer()?;
        let state_dir = self.files_state_dir()?;
        let held = crate::overlay_files::held(&state_dir, &self.domain, actor, path)
            .map_err(|e| self.files_io(path, e))?;
        // **A path this actor has already deleted is a miss**, not a second
        // deletion, and the reason is that this view has to agree with itself:
        // a read there answers `NotFound` and so does the size, so a delete
        // that answered success would be the one operation of the three still
        // claiming the path is there. It is the answer a direct domain gives on
        // the second delete of one file, for the same reason.
        if held == crate::overlay_files::Held::Tombstone {
            return Err(EngineError::NotFound(crate::engine::missing_attachment(
                &self.domain,
                path,
            )));
        }
        let base_holds = match self.engine.attachment_delete_size(&self.domain, path).await {
            Ok(_) => true,
            Err(EngineError::NotFound(_)) => false,
            Err(e) => return Err(e),
        };
        if !base_holds && held == crate::overlay_files::Held::Nothing {
            return Err(EngineError::NotFound(crate::engine::missing_attachment(
                &self.domain,
                path,
            )));
        }
        let done = if base_holds {
            crate::overlay_files::tombstone(&state_dir, &self.domain, actor, path)
        } else {
            crate::overlay_files::clear(&state_dir, &self.domain, actor, path)
        };
        done.map_err(|e| self.files_io(path, e))
    }

    /// Whose files overlay a write on this view lands in.
    ///
    /// One hop through [`crate::overlay_files::target_actor`], which is where
    /// the join that will one day answer differently belongs - see its doc.
    ///
    /// [`EngineError::Internal`] on the base view rather than
    /// [`DomainView::writing_actor`]'s [`OVERLAY_NEEDS_IDENTITY`]: a base view
    /// reaching a files-overlay write is a routing bug in this crate, and a
    /// sentence teaching a person how to connect would be teaching them to fix
    /// something that is not theirs.
    fn files_writer(&self) -> Result<&str> {
        let Some(actor) = self.actor.as_deref() else {
            return Err(EngineError::Internal(format!(
                "a files overlay write reached the base view of '{}'; a write with no actor \
                 belongs in the folder, not in an overlay",
                self.domain
            )));
        };
        Ok(crate::overlay_files::target_actor(
            actor,
            self.joined.as_deref(),
        ))
    }

    /// This actor's own files overlay entries, files and deletions alike,
    /// ordered by path, **with the honesty flag beside them**. Empty and
    /// certain on the base view, which holds none by definition.
    ///
    /// The seam the lifecycle reads: a share stages these over the base
    /// snapshot, the fold applies them to the folder, the counts name them
    /// beside the drafted rows. The flag travels with them because a removal
    /// gate has to tell "this actor holds nothing" from "nothing could be
    /// read", and a seam that had already thrown it away would force its
    /// callers back to the substrate to get it.
    pub(crate) fn files(&self) -> Result<crate::overlay_files::FileRead> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(crate::overlay_files::FileRead::default());
        };
        let state_dir = self.files_state_dir()?;
        Ok(crate::overlay_files::entries(
            &state_dir,
            &self.domain,
            actor,
        ))
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
    pub(crate) fn address_map<'b>(
        folding: &[&ActorDrafts],
        base: &'b [EngramDescriptor],
    ) -> HashMap<&'b str, &'b str> {
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

    /// Stages the tree one actor's share of a reviewing domain is detected against:
    /// the base snapshot's own content, with that actor's overlay rows laid over it
    /// - a draft as the file's content, a tombstone as the file's absence.
    ///
    /// **The base snapshot is one side, never the folder.** Its content lives under
    /// the state directory as the copies a pull recorded
    /// ([`state::read_base_file`]), and reading those rather than the files beside
    /// them is what makes "exactly this actor's draft" true: a stray direct edit of
    /// the reviewed folder is nobody's draft and takes no part in any share. Every
    /// path a pull records is written to both in lockstep, so a recorded path with
    /// no copy is a damaged state directory and is refused by name rather than
    /// quietly read off the folder.
    ///
    /// **`entries` are index rows, never the journal beside them.** The journal
    /// under the state directory is the durable mirror a rebuilt index is restored
    /// from, and a mirror that had fallen behind would quietly change what a share
    /// carries. The index is the live truth, so the index is what the caller reads.
    ///
    /// Nothing is cleared: a shared draft is still a draft, and the entries stay
    /// exactly as they stand until the merged work is pulled back and convergence
    /// takes them out.
    pub(crate) async fn materialise(
        &self,
        state_dir: &Path,
        base: &BTreeMap<String, BaseStamp>,
    ) -> Result<OverlayStaging> {
        let domain = self.domain.as_str();
        // **The rows, never the journal beside them**, and **the read-only id
        // lookup**, never an upserting one: a share of a domain this index has
        // never been told about holds no drafts, and asking must not register
        // one.
        let entries = match self.actor.as_deref() {
            Some(actor) => {
                let store = self.engine.store();
                let store = store.lock().await;
                match store.domain_id(domain).await? {
                    Some(domain_id) => store.overlay_entries(domain_id, actor).await?,
                    None => Vec::new(),
                }
            }
            None => Vec::new(),
        };
        let entries = entries.as_slice();
        let staging = OverlayStaging::at(state_dir.join(OVERLAY_STAGING_DIR));
        // A tree a previous run left behind - a process killed between the share and
        // the guard's own cleanup - is not a tree this share may inherit.
        match std::fs::remove_dir_all(staging.root()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(EngineError::Io {
                    path: staging.root().display().to_string(),
                    source,
                });
            }
        }
        std::fs::create_dir_all(staging.root()).map_err(|source| EngineError::Io {
            path: staging.root().display().to_string(),
            source,
        })?;
        for rel in base.keys() {
            if !is_within_domain(rel) {
                continue;
            }
            let Some(content) = state::read_base_file(state_dir, rel)? else {
                return Err(EngineError::Conflict(format!(
                    "domain '{domain}' records '{rel}' in its base snapshot but keeps no copy of it, \
                     so a share cannot say what the team's own version is. Its origin state is \
                     damaged: resync the domain by removing it and adding it from its origin again, \
                     then share"
                )));
            };
            write_staged_file(staging.root(), rel, &content)?;
        }
        for entry in entries {
            // The write verbs normalize a draft's path before it becomes a row, so
            // this is the second assertion rather than the first - and it is here
            // because a path that escaped would write outside the staged tree, which
            // is the one failure a share could not recover from. The rule is the one
            // every write and move verb holds a path to, so a filename a person
            // chose - `notes/plan: v2.md` and its like - is a path like any other.
            if !is_within_domain(&entry.path) {
                return Err(EngineError::Conflict(format!(
                    "draft '{}' in domain '{domain}' stands at a path that is not inside the domain, \
                     so it cannot be shared",
                    entry.path
                )));
            }
            if entry.tombstone {
                let path = join_rel(staging.root(), &entry.path);
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: path.display().to_string(),
                            source,
                        });
                    }
                }
            } else {
                write_staged_file(staging.root(), &entry.path, entry.content.as_bytes())?;
            }
        }
        self.stage_files(&staging)?;
        Ok(staging)
    }

    /// Lay this actor's files overlay over the staged tree, after the rows: a
    /// file they wrote becomes the file at that path, a deletion marker takes
    /// the staged file away.
    ///
    /// **Nothing here is ever skipped in silence**, which is the one property
    /// this pass is built around. `ops::propose` detects the proposal against
    /// this tree, so a file left out of it does not read as "not shared" - it
    /// reads as a file the actor deleted, and the proposal would ask the team
    /// to delete their own copy of it. An unreadable folder and an entry whose
    /// bytes have gone are therefore both refusals naming the path, never an
    /// omission.
    ///
    /// The base snapshot above already carries the team's own attachments (a
    /// pull records every path it applies, not only the markdown), so this pass
    /// is the actor's half and nothing else.
    fn stage_files(&self, staging: &OverlayStaging) -> Result<()> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return Ok(());
        };
        let files = self.files()?;
        if files.unreadable {
            return Err(EngineError::Io {
                path: format!("the files overlay of '{domain}' for '{actor}'"),
                source: std::io::Error::other(
                    "a share carries every file its author drafted, and one that could not be \
                     listed would be proposed as a deletion of the team's own copy",
                ),
            });
        }
        let state_dir = self.files_state_dir()?;
        for entry in &files.entries {
            // The second assertion rather than the first, exactly as the rows
            // pass above makes it and for the same reason: a path that escaped
            // would write outside the staged tree, which is the one failure a
            // share could not recover from. It holds by construction here -
            // `overlay_files::collect` drops anything `validate_asset_path`
            // rejects - and the rows pass has the same guarantee from its write
            // verbs and asserts anyway.
            if !is_within_domain(&entry.path) {
                return Err(EngineError::Conflict(format!(
                    "the draft file '{}' in domain '{domain}' stands at a path that is not \
                     inside the domain, so it cannot be shared",
                    entry.path
                )));
            }
            if entry.tombstone {
                let path = join_rel(staging.root(), &entry.path);
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: path.display().to_string(),
                            source,
                        });
                    }
                }
                continue;
            }
            let bytes = crate::overlay_files::read(&state_dir, domain, actor, &entry.path)
                .map_err(|e| self.files_io(&entry.path, e))?
                .ok_or_else(|| EngineError::Io {
                    path: format!("the files overlay of '{domain}' at '{}'", entry.path),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "the file was listed and then could not be read, and a share that \
                             dropped it would propose a deletion of the team's copy",
                    ),
                })?;
            write_staged_file(staging.root(), &entry.path, &bytes)?;
        }
        Ok(())
    }
}
