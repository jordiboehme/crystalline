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

use crystalline_core::{CrystallineUrl, parse_engram};
use crystalline_index::{
    AttachmentRow, BrowseLevel, DomainId, EngramDescriptor, EngramId, EngramRecord, EngramSummary,
    RecentFilter, Store, StoredEngram,
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
pub(crate) struct DomainView<'a> {
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
    pub(crate) fn for_read(
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
    /// crates/service/tests/overlay_domains.rs scans for exactly that.
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
    fn writing_actor(&self) -> Result<&str> {
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
            Err(e) => Some(unmirrored(domain, actor, path, &e)),
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
    pub(crate) async fn drop(&self, domain_id: DomainId, path: &str) -> Result<()> {
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
        store.clear_overlay_entry(domain_id, actor, path).await?;
        Ok(())
    }

    /// [`Engine::resolve_in`] for a call that may be acting inside a draft
    /// overlay: what THIS actor sees at that identifier.
    ///
    /// `None` is the base resolution unchanged, so a direct domain reaches
    /// exactly the code it always did. With an actor there are three
    /// differences, and each one is a place a draft would otherwise be
    /// invisible to its own author:
    ///
    /// * a path this actor has tombstoned resolves to nothing, in the same
    ///   words an engram nobody wrote produces - their deletion is a deletion
    ///   for them;
    /// * a draft at a path no base row holds resolves through the draft's own
    ///   row, which is the only way an engram created in review mode can be
    ///   edited, moved or deleted at all;
    /// * a draft over a base row resolves to the BASE descriptor. Its path,
    ///   domain and permalink are what a write needs, and the draft's own row
    ///   is read by the arm that writes it, so resolving to the base keeps one
    ///   engram one address whether or not this actor has started drafting it.
    pub(crate) async fn resolve(
        &self,
        identifier: &str,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        let domain = self.domain.as_str();
        let Some(actor) = self.actor.as_deref() else {
            return self.engine.resolve_in(identifier, domain).await;
        };
        match self.engine.resolve_in(identifier, domain).await {
            Ok((desc, source)) => {
                let store = self.engine.store();
                let held = {
                    let store = store.lock().await;
                    store
                        .overlay_entry(desc.domain_id, actor, &desc.path)
                        .await?
                };
                if held.is_some_and(|entry| entry.tombstone) {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{identifier}' in domain '{domain}'"
                    )));
                }
                Ok((desc, source))
            }
            // The base knows nothing about this identifier, which is exactly
            // the case a draft-only engram is in. The miss is kept and raised
            // unchanged when the overlay knows nothing either, so an
            // identifier nobody wrote reads the same in both modes.
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
    pub(crate) async fn move_within(
        &self,
        p: &MoveParams,
        src: &EngramDescriptor,
        src_source: &ContentSource,
        dest_rel: &str,
        cross: bool,
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
        if dest_rel == src.path {
            return Err(EngineError::Invalid(
                "the destination is where the engram already is".into(),
            ));
        }
        // Free in THIS actor's view, which is the only view the move happens
        // in: a path another actor is drafting at is not taken for this one,
        // and a base row at the destination is, since the moved engram would
        // shadow it rather than land beside it.
        {
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
        // frontmatter never carried one and the path's own slug is it.
        let dest_permalink = parse_engram(&text)
            .map(|engram| {
                EngramRecord::from_engram(&engram, dest_rel, virtual_stamp(&text)).permalink
            })
            .unwrap_or_else(|_| src.permalink.clone());
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
                self.drop(src.domain_id, &src.path).await?;
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
                    self.drop(src.domain_id, &src.path).await
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
        let mut receipt = json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": p.domain, "permalink": dest_permalink, "path": dest_rel },
            "cross_domain": false,
            "links_rewritten": 0,
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

    /// The dangling references in one actor's own drafts, as
    /// [`crystalline_index::UnresolvedRef`] rows to stand beside the base ones.
    ///
    /// Two halves, and both are the same sentence read from different ends: the
    /// references come off the draft's own row ([`Store::outbound_refs`], the
    /// call `read_engram` makes for a draft's outbound edges), and whether each
    /// one answers to anything is decided against `descs` - the shadowed
    /// listing, which is base rows plus this actor's drafts with their deleted
    /// paths already absent. The stored `resolved` flag cannot answer it: every
    /// arm of the index's resolution is `actor = ''` by design, so it calls a
    /// link between two of one author's drafts broken and a link to a path they
    /// have deleted sound, and both are backwards for the person reading.
    ///
    /// A reference into ANOTHER domain keeps the stored verdict. That domain's
    /// rows are not in this listing, and a resolved edge there is a fact about
    /// the domain rather than about a reader - which is the rule that stays,
    /// deliberately, whatever this sweep does inside its own domain.
    ///
    /// Rows come back ordered the way both backends order the base rows - by
    /// path, then line, then kind, then target (`turso/mod.rs` `ORDER BY 7, 6,
    /// 2, 5`, and the matching Postgres form) - because `outbound_refs` orders
    /// by line alone, which leaves two references on one line to the union's
    /// own arm order. The `links_to` default for a prose wikilink is
    /// [`crystalline_index::LINKS_TO`], the constant both backends spell into
    /// their own queries, so the two never drift apart.
    pub(crate) async fn draft_unresolved(
        &self,
        store: &dyn Store,
        descs: &[EngramDescriptor],
        drafts: &HashMap<String, String>,
    ) -> Result<Vec<crystalline_index::UnresolvedRef>> {
        let domain = self.domain.as_str();
        if drafts.is_empty() {
            return Ok(Vec::new());
        }
        // The two readings the index tries inside one domain: the target as a
        // permalink, then as a title, the second case-insensitively.
        let permalinks: HashSet<&str> = descs.iter().map(|d| d.permalink.as_str()).collect();
        let titles: HashSet<String> = descs.iter().map(|d| d.title.to_lowercase()).collect();

        let mut held: Vec<&EngramDescriptor> = descs
            .iter()
            .filter(|d| drafts.contains_key(&d.path))
            .collect();
        held.sort_by(|a, b| a.path.cmp(&b.path));

        let mut out: Vec<crystalline_index::UnresolvedRef> = Vec::new();
        for d in held {
            let mut refs: Vec<crystalline_index::UnresolvedRef> = Vec::new();
            for reference in store.outbound_refs(d.id).await? {
                let answered = match reference.to_domain.as_deref() {
                    Some(named) if named != domain => reference.resolved,
                    _ => {
                        permalinks.contains(reference.to_target.as_str())
                            || titles.contains(&reference.to_target.to_lowercase())
                    }
                };
                if answered {
                    continue;
                }
                refs.push(crystalline_index::UnresolvedRef {
                    from: d.id,
                    rel_type: reference
                        .rel_type
                        .unwrap_or_else(|| crystalline_index::LINKS_TO.to_string()),
                    kind: reference.kind,
                    target_domain: reference.to_domain,
                    target: reference.to_target,
                    line: Some(reference.line),
                });
            }
            refs.sort_by(|a, b| {
                a.line
                    .cmp(&b.line)
                    .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
                    .then_with(|| a.target.cmp(&b.target))
            });
            out.extend(refs);
        }
        Ok(out)
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

    /// Whether this reader still sees something at a path the base holds:
    /// `false` when their own tombstone deletes it, which is the one edit a
    /// resolver has to make once the base row is already in hand.
    pub(crate) async fn exists(&self, domain_id: DomainId, path: &str) -> Result<bool> {
        let Some(actor) = self.actor.as_deref() else {
            return Ok(true);
        };
        let store = self.engine.store();
        let store = store.lock().await;
        Ok(!store
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

    /// Every attachment this reader sees in the domain, metadata only.
    ///
    /// **The same rows every other reader sees, and that is the answer rather
    /// than an omission.** The attachment table carries no actor dimension, so
    /// one actor's view of a domain's attachments IS its base attachments; an
    /// attachment is shared state and a reference to one is asked of the union.
    /// The operation exists so a later attachment overlay has one seam to land
    /// in rather than five call sites to find.
    pub(crate) async fn attachments(&self) -> Result<Vec<AttachmentRow>> {
        self.engine.attachment_list(&self.domain).await
    }

    /// One attachment's bytes and its metadata row, from the same substrate
    /// [`DomainView::attachments`] lists.
    pub(crate) async fn attachment_bytes(&self, path: &str) -> Result<(Vec<u8>, AttachmentRow)> {
        self.engine.attachment_read(&self.domain, path).await
    }

    /// What deleting one attachment would take away, for a preview that must
    /// never be stricter than the act it previews.
    pub(crate) async fn attachment_delete_size(&self, path: &str) -> Result<u64> {
        self.engine.attachment_delete_size(&self.domain, path).await
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
        Ok(staging)
    }
}
