use super::*;
use crate::collab::session::{ComposeRefused, MoveClose, MovedTo, RoomDoc};

/// What [`Engine::relink_engram`] rewrote in one linking engram.
pub(super) struct Relinked {
    /// How many references changed.
    pub(super) references: usize,
    /// The linker's permalink.
    pub(super) permalink: String,
    /// Whether the rewrite composed into an open room rather than the file
    /// or the row.
    pub(super) live: bool,
}

/// An engram file that moved. A relative Markdown link names a file, not an
/// address, so the links to it follow the file whether or not its permalink
/// changed: inside the domain by a new relative path, and into another
/// domain (`across`) by its new address, since a relative path never reaches
/// into another domain.
#[derive(Debug, Clone, Copy)]
pub(super) struct Relocation<'a> {
    /// The domain it moved from.
    pub(super) domain_id: DomainId,
    /// Its domain-relative path before the move.
    pub(super) from: &'a str,
    /// Its domain-relative path after it.
    pub(super) to: &'a str,
    /// For a move into another domain, the `domain:permalink` address the
    /// source domain's links to it name afterwards.
    pub(super) across: Option<&'a str>,
    /// The domain it lives in after the move (the one it moved from, when
    /// it stayed).
    pub(super) to_domain: &'a str,
    /// Every spelling of the domain it moved from, which a wikilink spelled
    /// as its path may carry as a prefix (`[[notes:guides/a.md]]`).
    pub(super) spellings: &'a HashSet<String>,
}

/// How a move into another domain rewrites the moved engram's own links,
/// worked out from the source domain before the move.
#[derive(Debug, Default)]
struct CrossLinks {
    /// Each relative link target, as written, and what replaces it.
    relative: HashMap<String, crystalline_core::relink::NoteLinkTo>,
    /// Each wikilink, by its domain prefix (none for a bare one) and target
    /// as written, that resolved in the source domain and would be read in
    /// the destination once it lands, and the address that keeps it on what
    /// it reached. Applied before the engram's references to itself follow
    /// it, so a self-link's new permalink is never mistaken for one of these.
    readdressed: HashMap<(Option<String>, String), CrossTarget>,
    /// Each prefixed wikilink whose prefix spells the destination, by that
    /// prefix and its target: written bare once the engram lands there.
    collapsed: HashSet<(String, String)>,
    /// The links left as written that would land on an engram in the
    /// destination, as they were written.
    stranded: Vec<String>,
    /// The target text, with its `#heading`, of every reference of the moved
    /// engram the index could not resolve before the move, which tells a link
    /// that was dead already from one the move broke (see
    /// [`Engine::dead_links`]).
    unresolved: HashSet<String>,
}

/// What the whole text of a wikilink whose title holds a `#` reached as a
/// title in the domain an engram leaves.
#[derive(Debug)]
enum TitleHit {
    /// Another engram, and the address it is written with from now on.
    Other(CrossTarget),
    /// The moving engram itself.
    Itself,
    /// Nothing: the text is read split, as a target and a heading.
    Nothing,
}

/// What one wikilink of an engram moving into another domain is written as.
#[derive(Debug)]
struct CrossTarget {
    /// The target part: `domain:permalink`, or a path for the engram itself.
    to: String,
    /// The words of a link that named its target by title, kept as the
    /// display text when the link shows none, so the reader sees what the
    /// author wrote while the link names the permalink.
    words: Option<String>,
}

impl CrossTarget {
    /// The new target part of a link, `shown` when it carries a display text
    /// of its own.
    fn written(&self, shown: bool) -> String {
        match &self.words {
            Some(words) if !shown => format!("{}|{words}", self.to),
            _ => self.to.clone(),
        }
    }
}

/// The `links_failed` reason for the links of the moved engram a move into
/// another domain could not give an address, or gave one without its
/// `#fragment` or title.
fn cross_link_reason(
    stranded: &[String],
    unwritable: &[String],
    dropped: &[String],
) -> Option<String> {
    let mut parts = Vec::new();
    if !stranded.is_empty() {
        parts.push(format!(
            "these links leave the domain they were written in and now land on another engram, \
             so fix them by hand: {}",
            stranded.join(", ")
        ));
    }
    if !unwritable.is_empty() {
        parts.push(format!(
            "these links could not be written as a wikilink and were left as they were: {}",
            unwritable.join(", ")
        ));
    }
    if !dropped.is_empty() {
        parts.push(format!(
            "these links now name the engram by its address and lost their #fragment or title: {}",
            dropped.join(", ")
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The `links_failed` reason for a linking engram in a domain the mover may
/// see but only read: its link was left as it was.
const LINKER_READ_ONLY: &str =
    "you may only read this domain, so its link to the moved engram was left as it was";

/// Name one linking page or engram a move could not rewrite, in the shape
/// every move receipt's `links_failed` carries: `{ domain, path, reason }`.
/// Shared by the engram move, the reflection move and the reflection file
/// move, so the three receipts never drift apart. The caller logs it with
/// the context it has.
pub(super) fn note_link_failure(
    failed: &mut Vec<Value>,
    domain: &str,
    path: &str,
    e: &EngineError,
) {
    failed.push(json!({ "domain": domain, "path": path, "reason": e.to_string() }));
}

impl Engine {
    // --- move ----------------------------------------------------------------

    /// Move an engram to a new path, a new permalink or a new domain, and
    /// rewrite every reference that followed it there.
    ///
    /// A move is a refactoring. Source and destination may each be a file or
    /// virtual domain, so a move carries content between the two truths: a
    /// same-domain move renames the row in place (the id, and with it every
    /// chunk, embedding and bound reference, stays), a cross-domain move reads
    /// the source content and re-indexes it into the destination's source.
    ///
    /// **The permalink** is decided by `Engine::moved_permalink`: by default
    /// it follows the move only when it was in step with the old path, so a
    /// deliberate custom permalink survives a re-filing; `MoveParams::permalink`
    /// asks for the path's own, the current one, or a named one. A destination
    /// equal to the current path is the permalink-only rename - the repair for
    /// a permalink that drifted off its folder - and a move that changes
    /// neither path, domain nor permalink is refused as the no-op it is. The
    /// new permalink is written into the frontmatter `permalink:` line
    /// surgically, so the file and the row say the same thing and a later
    /// sync does not quietly put the old one back; `recorded_at` stays, and the
    /// `generated` block records the mover whenever the move changed the text.
    ///
    /// **Every address change rewrites every reference**, whether the domain,
    /// the permalink or both changed: `[[old]]`, `[[domain:old]]`, relation
    /// bullets and `crystalline://domain/old` URLs (see
    /// [`crystalline_core::relink`]), found through the index before the move
    /// and rewritten in each referencing engram's source of truth afterwards.
    /// A link by title is left alone unless the engram changed domain, since
    /// the title did not change and the link still resolves. `update_links`
    /// set to false skips the rewrite, as it always did.
    ///
    /// `scope` bounds the *side effect*, which is the half a surface cannot
    /// gate for itself. Whether this caller may write either end is decided at
    /// the edge (the REST write gate, the MCP domain gate); what only this
    /// function can decide is which other domains it rewrites a link inside.
    /// Those referencing engrams live in domains the mover may never have been
    /// shown. So the rewrite skips a domain this caller may not see: its link
    /// is left as it was - dangling, which its own members see as an
    /// unresolved-reference finding on the next sweep - rather than silently
    /// edited by somebody with no access to it. A domain the caller sees but
    /// may only read is skipped the same way and named in `links_failed`: a
    /// move never writes where its mover may not write. The receipt's counts
    /// (`links_rewritten`, the engrams rewritten, and `references_rewritten`,
    /// the references inside them) cover the visible rewrites only, so a
    /// receipt never counts a file its reader may not know exists.
    pub async fn move_engram(&self, p: &MoveParams, scope: &crate::scope::Scope) -> Result<Value> {
        self.move_engram_as(p, None, scope).await
    }

    /// A move inside this actor's draft, under its two draft locks: its own
    /// function, so the locks end with it and every later step of
    /// [`Engine::move_engram_as`] (which closes rooms) runs with none held.
    async fn move_draft_locked(
        &self,
        view: &DomainView<'_>,
        p: &MoveParams,
        draft: DraftMove<'_>,
    ) -> Result<Value> {
        let DraftMove {
            who,
            src,
            src_source,
            dest_rel,
            cross,
            asked,
            mover,
        } = draft;
        // Two writes - a tombstone where the team's file is, an entry at
        // the destination - so two draft locks, taken in key order so two
        // moves that cross each other cannot each hold what the other
        // wants. `move_within` refuses a cross-domain move outright, so
        // both paths are this actor's own drafts in this one domain. See
        // `Engine::draft_lock`, whose keys these are.
        //
        // Through `lock_files`, which compares the keys: a move onto its own
        // path is the permalink-only rename, and on a disk that folds case
        // `Note.md` and `note.md` are two strings with one key once the mirror
        // exists. Either way that is one lock, taken once, since asking for
        // it twice would wait for ever.
        let state_dir = self.journal_state_dir()?;
        let mirror = |path: &str| {
            crate::overlay_journal::entry_path(&state_dir, &p.domain, who, path)
                .map_err(|e| EngineError::Invalid(e.to_string()))
        };
        let (source, destination) = (mirror(&src.path)?, mirror(dest_rel)?);
        let _locks = self.lock_files(&[&source, &destination]).await;
        view.move_within(p, src, src_source, dest_rel, cross, asked, mover)
            .await
    }

    /// [`Engine::move_engram`] with the mover's identity, resolved by
    /// [`Engine::actor`] (or, for a draft, the composed identity) and written
    /// into the moved engram's `generated` block when the move changes its
    /// text. The engrams whose references are rewritten record Crystalline
    /// itself instead: the mover did not author them.
    pub async fn move_engram_as(
        &self,
        p: &MoveParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // One engram move at a time in this process. A move gathers the
        // engrams that link it before it writes and rewrites them after it
        // moved; two moves running at once could each find a linker the
        // other has just moved away, and leave a link to the old address.
        // Taken first, before any write ticket, so a move waiting here holds
        // nothing a rename or another move waits for.
        let _one_move = self.lock_moves().await;
        // Resolved once, before anything is written, and used twice below: to
        // look the destination up, and to bound the inbound rewrite.
        let hidden = self.hidden_for(scope).await?;
        let p = &self.localized(p, &hidden).await;
        let view = DomainView::for_write(self, &p.domain, scope).await?;
        let overlay = view.actor();
        let (src, src_source) = match view.resolve(&p.identifier).await {
            Ok(resolved) => resolved,
            Err(EngineError::NotFound(missing)) => {
                self.refuse_reflection_identifier(&p.domain, &p.identifier)
                    .await?;
                return Err(EngineError::NotFound(missing));
            }
            Err(e) => return Err(e),
        };
        // A hit can be a stale row: a file that became the Reflection's page
        // since the last sync still has its engram row until the next one.
        self.refuse_reflection_identifier(&p.domain, &src.path)
            .await?;
        let dest_domain = p
            .destination_domain
            .clone()
            .unwrap_or_else(|| p.domain.clone());
        // Scoped, and that is load bearing rather than tidy. This lookup raises
        // the one error that names every registered domain, and a surface gate
        // above it can only check the spelling it normalizes: a padded or empty
        // `destination_domain` passes a gate that trims or skips it and arrives
        // here verbatim. Resolving it against the caller's own visible set
        // closes that for every spelling, present and future, instead of asking
        // one more pair of normalizers to agree.
        let dest_source = self.content_source_scoped(&dest_domain, &hidden)?;
        let dest_rel = normalize_md(&p.destination);
        if dest_rel.is_empty() {
            return Err(EngineError::Invalid("destination path is empty".into()));
        }
        // As on the create path: `normalize_md` drops empty segments but keeps
        // `..`, so containment is decided here, before either reserved check
        // reads the destination as text and before `join_rel` builds a path
        // from it.
        if !is_within_domain(&dest_rel) {
            return Err(EngineError::Invalid(escapes_root_error(&dest_rel)));
        }
        if crystalline_core::is_reserved_path(&dest_rel) {
            return Err(EngineError::Invalid(reserved_name_error(&dest_rel)));
        }
        // The declared switch of the destination domain: an engram may move
        // out of a folder whose switch waits for it, never into one.
        refuse_reserved_destination(
            &dest_rel,
            ReflectionMode::declared(self.reflection_switch(&dest_domain)),
        )?;
        let cross = dest_domain != p.domain;
        // A move into another domain writes there too, so it is counted there
        // for the whole move: a rename of the destination waits for it, and a
        // destination a rename has paused is waited for first.
        let _dest_ticket = if cross {
            Some(self.enter_write(&dest_domain).await?)
        } else {
            None
        };
        // The source was checked by its write view; a move into another
        // domain writes there too, so the destination is asked the same way,
        // inside its ticket and before anything is written at either end.
        if cross {
            self.refuse_read_only_domain(&dest_domain, scope).await?;
        }
        let new_permalink = Self::moved_permalink(&src, &dest_rel, p.permalink.as_deref())?;
        let in_place = !cross && dest_rel == src.path;
        // A file domain's MANIFEST never leaves its place, in a draft either:
        // the domain needs it. A rename of its permalink in place keeps it.
        if !in_place {
            refuse_manifest_removal(&src_source, &src.path)?;
        }
        if in_place && new_permalink == src.permalink {
            return Err(EngineError::Invalid(format!(
                "'{}' already sits at '{dest_rel}' and answers to '{new_permalink}', so this \
                 move changes nothing; to rename it in place pass a permalink (\"path\" for the \
                 destination path's own)",
                src.permalink
            )));
        }
        // Whether the engram answers to a different address afterwards, which
        // is what every reference to it has to follow.
        let readdressed = cross || new_permalink != src.permalink;
        // Whether its file moved inside the domain, which is what a relative
        // link to it, and each of its own, has to follow. A move into another
        // domain leaves relative links as they were: a relative path never
        // reaches into another domain.
        let relocated = !cross && dest_rel != src.path;

        // A move onto the MANIFEST writes the moved text as the MANIFEST, so it
        // meets the switch-on rule every MANIFEST writer meets (decision D10),
        // before anything is written at either end and before the review-mode
        // branch, which writes a draft of it. A taken MANIFEST.md is refused
        // later as a collision; this is the move onto a missing one. The name
        // is compared in any letter case, as the write paths do: a
        // case-insensitive filesystem (the macOS and Windows defaults) opens
        // `manifest.md` as the very `MANIFEST.md` routing reads.
        // The text the screens below read, kept so the move can tell whether
        // the text it reads again under its locks is still that text.
        let mut screened: Option<String> = None;
        if names_the_manifest(&dest_rel) {
            // In a reviewing domain the move writes the mover's draft, so the
            // text that lands is what they see: their own draft when they hold
            // one, the base text otherwise. The miss below is reached only when
            // the mover's tombstone lands between `view.resolve` above and this
            // read; otherwise the resolve answers the same miss first.
            let text = match overlay {
                Some(_) => view.text_at(&src_source, &src).await?.ok_or_else(|| {
                    EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        p.identifier, p.domain
                    ))
                })?,
                None => self.move_source_text(&src_source, &src, &p.domain).await?,
            };
            self.refuse_read_only_switch_on(&dest_domain, &text, scope)
                .await?;
            // No MANIFEST stands there before the move.
            self.screen_manifest_write(&dest_domain, Some(""), &text)
                .await?;
            screened = Some(text);
        }

        // The third place a move can land, and it is two writes rather than
        // one: a tombstone where the team's file is, so this actor stops
        // seeing the engram there, and an entry at the destination, so they
        // see it where they moved it to. The folder itself does not move,
        // which is the whole of review mode - the rename is reviewed like any
        // other change.
        if let Some(who) = overlay {
            // A draft's permalink changes only when the caller names one. The
            // default rule stays out of review mode on purpose: a moved draft
            // is paired with the team's engram it came from by the address it
            // carries (the fold and the pull's rename-follow both key on it),
            // so a draft that quietly took the path's slug would stop reading
            // as a move of that engram and start reading as a new one.
            let asked = p
                .permalink
                .as_deref()
                .filter(|asked| !asked.trim().is_empty())
                .map(|_| new_permalink.as_str());
            let mover = self.actor_for(client, overlay);
            let draft = DraftMove {
                who,
                src: &src,
                src_source: &src_source,
                dest_rel: &dest_rel,
                cross,
                asked,
                mover: &mover,
            };
            // A draft move that names a new permalink changes the address
            // the mover's own room over the draft saves to, so that room is
            // ended and fenced like a base room (see below), once the
            // screens a draft move can be refused by have passed. A draft
            // moves only inside its domain (`move_within`).
            let readdressing = !cross && asked.is_some_and(|asked| asked != src.permalink);
            let closing = if readdressing {
                if dest_rel != src.path {
                    view.refuse_taken_draft_destination(p, &src, &dest_rel)
                        .await?;
                }
                self.close_room_for_move(
                    &p.domain,
                    RoomDoc::Engram(src.permalink.clone()),
                    Some(who),
                    MovedTo {
                        domain: None,
                        path: dest_rel.clone(),
                        permalink: Some(new_permalink.clone()),
                    },
                )
                .await
            } else {
                None
            };
            let moved = self.move_draft_locked(&view, p, draft).await?;
            if let Some(closing) = closing {
                closing.moved().await;
            }
            return Ok(moved);
        }

        // A move onto the MANIFEST may move the destination's switch.
        let switch_before =
            names_the_manifest(&dest_rel).then(|| self.reflection_switch(&dest_domain));

        // After the overlay branch, so a move OUT of a reviewing domain keeps
        // its own refusal ("share the change first"), and before anything is
        // read or written, so a refusal costs nothing. This is the move INTO
        // one: the view above is the SOURCE domain's, so a move from a domain
        // that takes changes directly would otherwise write the destination's
        // folder - the engram and every attachment it carries.
        if cross {
            self.refuse_write_into_reviewed_folder(
                &dest_domain,
                "a move from another domain cannot land there",
            )?;
        }

        // Destination collision checks, all of them before the first write so a
        // refusal leaves nothing half done: the path on disk or in the
        // database (an in-place rename is its own destination), then the
        // address, which the unique index would otherwise refuse only at the
        // reindex - after the file had already landed.
        if !in_place {
            self.ensure_dest_free(
                &dest_source,
                &dest_domain,
                &dest_rel,
                &src_source,
                &src.path,
                cross,
            )
            .await?;
        }
        if readdressed {
            self.refuse_permalink_held(&dest_domain, &new_permalink, &src, cross)
                .await?;
        }

        // Gather the referencing engrams before the move, while every bound
        // reference's `to_id` still points at the source row.
        let update_links = p.update_links.unwrap_or(true);
        // A move into another domain: the source domain's links that bind by
        // path name the engram's new address instead.
        let across = format!("{dest_domain}:{new_permalink}");
        let src_spellings: HashSet<String> = if (relocated || cross) && update_links {
            self.name_table_now()
                .await
                .spellings()
                .into_iter()
                .filter(|(_, local)| *local == p.domain)
                .map(|(spelling, _)| spelling)
                .collect()
        } else {
            HashSet::new()
        };
        let relocation = ((relocated || cross) && update_links).then_some(Relocation {
            domain_id: src.domain_id,
            from: &src.path,
            to: &dest_rel,
            across: cross.then_some(across.as_str()),
            to_domain: &dest_domain,
            spellings: &src_spellings,
        });
        let linkers = if (readdressed || relocated) && update_links {
            self.move_linkers(&src, &hidden).await?
        } else {
            Vec::new()
        };
        // A domain the mover may see but only read is left as it was, like a
        // hidden one: a move must not write where its mover may not write
        // (on a domain GitHub governs, that domain is later shared through
        // the App). Unlike a hidden domain it is named in `links_failed`,
        // since the mover may know it exists. Both ends passed the write
        // gate at the edge.
        let mut read_only_linkers: HashSet<String> = HashSet::new();
        let mut asked: HashSet<&str> = HashSet::new();
        for linker in &linkers {
            let domain = linker.domain.as_str();
            if domain == p.domain || domain == dest_domain || !asked.insert(domain) {
                continue;
            }
            if matches!(
                self.write_access_block(scope, domain).await?,
                Some(WriteBlock::Role | WriteBlock::WriteList { .. })
            ) {
                read_only_linkers.insert(domain.to_string());
            }
        }
        // Every other domain the rewrite will write into is counted for the
        // rest of the move, as the source and the destination are, so a
        // rename of a linker's domain waits for the move instead of racing
        // its rewrite. Entered once, up front, and never for a domain this
        // move already holds: a rename that paused it would wait for this
        // move while this move waited for the rename. A domain a rename has
        // paused is not waited for: moves run one at a time, so waiting
        // would hold every other move back for as long as the rename may
        // take. Its linkers are not written and are named in the receipt.
        let mut linker_tickets = Vec::new();
        let mut paused: HashMap<String, EngineError> = HashMap::new();
        for linker in &linkers {
            let domain = linker.domain.as_str();
            if domain == p.domain
                || domain == dest_domain
                || read_only_linkers.contains(domain)
                || paused.contains_key(domain)
                || linker_tickets.iter().any(|(held, _)| held == domain)
            {
                continue;
            }
            match self.try_enter_write(domain) {
                Ok(ticket) => linker_tickets.push((domain.to_string(), ticket)),
                Err(e) => {
                    paused.insert(domain.to_string(), e);
                }
            }
        }
        let spec = Relink {
            from_domain: &p.domain,
            from_permalink: &src.permalink,
            title: &src.title,
            to_domain: &dest_domain,
            to_permalink: &new_permalink,
            shadowed: &[],
        };

        // The source's and the destination's write locks, from the read of
        // the source through the write, the removal and the reindex, as every
        // other writer of either file takes them: an edit of the source
        // cannot land between the read and the removal and be lost, nor after
        // the removal and bring the source back. Both in lock key order; an
        // in-place move takes one. A virtual end takes none, since the store
        // writes it in one statement. Released before the attachments and the
        // linkers, so the rewrite below never holds two file locks.
        let src_abs = match &src_source {
            ContentSource::File { root } => Some(join_rel(root, &src.path)),
            ContentSource::Virtual => None,
        };
        let dest_abs = match &dest_source {
            ContentSource::File { root } => Some(join_rel(root, &dest_rel)),
            ContentSource::Virtual => None,
        };
        let held: Vec<&Path> = src_abs
            .iter()
            .chain(dest_abs.iter())
            .map(PathBuf::as_path)
            .collect();
        // A path-only move keeps the room over the engram (see below), and
        // while it is open its text IS the engram. The rebase of the
        // engram's own links is then composed into that room after the move,
        // the way a linker's rewrite is, rather than written to the file under
        // it, where the room's next save would meet it as a change that may
        // conflict with what is being typed. Asked before the file locks,
        // since the answer takes the room's lock, which comes first.
        let defer_rebase = match (relocation.is_some() && !readdressed, self.collab_rooms()) {
            (true, Some(rooms)) => rooms
                .live_text(&p.domain, &src.permalink, None)
                .await
                .is_some(),
            _ => false,
        };
        // A move that changes the address the editor saves to ends the room
        // over the engram: its text is saved first, and no room opens over
        // the old address until the move is done (the fence). Its sockets
        // are told where the engram went once the move has landed, and
        // `internal` if it fails from here. After every screen, so a refused
        // move leaves the room open; before the file locks, which a room's
        // save takes. A path-only move keeps the room: it saves by address,
        // and the address does not change.
        let mut closing = if readdressed {
            self.close_room_for_move(
                &p.domain,
                RoomDoc::Engram(src.permalink.clone()),
                None,
                MovedTo {
                    domain: cross.then(|| dest_domain.clone()),
                    path: dest_rel.clone(),
                    permalink: Some(new_permalink.clone()),
                },
            )
            .await
        } else {
            None
        };
        // The window R81 named, between the close and the locks: the fence
        // keeps a room from opening in it.
        self.take_room_screen_hold().await;
        let _lock = self.lock_files(&held).await;
        // The destination was free when it was checked above; it is asked
        // again now that nobody else may write it.
        if !in_place {
            self.ensure_dest_free(
                &dest_source,
                &dest_domain,
                &dest_rel,
                &src_source,
                &src.path,
                cross,
            )
            .await?;
        }

        // Read here for the write, inside the locks, by the strict reader the
        // MANIFEST check above uses too (see [`Engine::move_source_text`]).
        let original = self.move_source_text(&src_source, &src, &p.domain).await?;
        // The screens above read the source before these locks were taken, so
        // an edit may have landed in between. A text that changed since is
        // screened again here, where it can no longer change. The screens
        // take no file lock, so running them under these is safe.
        if let Some(screened) = &screened
            && *screened != original
        {
            self.refuse_read_only_switch_on(&dest_domain, &original, scope)
                .await?;
            self.screen_manifest_write(&dest_domain, Some(""), &original)
                .await?;
        }
        // A move writes the engram anew, so it lands as LF whatever line
        // endings it was read with, a plain rename included.
        let original = crystalline_core::to_lf(&original).into_owned();
        // Every linking engram a rewrite could not finish, and the moved
        // engram itself when its own links could not all follow it, as the
        // receipt's `links_failed` names them.
        let mut failed: Vec<Value> = Vec::new();
        let cross_links = if cross && update_links {
            let mut links = self
                .cross_links(&p.domain, &src.path, &dest_domain, &dest_rel, &original)
                .await?;
            let store = self.store.lock().await;
            links.unresolved = store
                .outbound_refs(src.id, None)
                .await?
                .into_iter()
                .filter(|r| !r.resolved)
                .map(|r| r.to_whole.unwrap_or(r.to_target))
                .collect();
            Some(links)
        } else {
            None
        };
        // The text the engram lands with, when the move has to change it: the
        // permalink line, and its own references to itself. `None` is the
        // plain rename, which carries the bytes across untouched.
        let own_shadowed = self.shadowed_titles(&original, &p.domain, &spec).await?;
        let own_spec = Relink {
            shadowed: &own_shadowed,
            ..spec
        };
        let (moved_text, own_notes) = Self::readdressed_text(
            &original,
            &dest_rel,
            &new_permalink,
            (readdressed && update_links).then_some(&own_spec),
            relocation
                .as_ref()
                .filter(|moved| !defer_rebase && moved.across.is_none()),
            cross_links.as_ref(),
            &self.actor(client),
        )?;
        if let Some(reason) = own_notes {
            failed.push(json!({ "domain": dest_domain, "path": dest_rel, "reason": reason }));
        }

        // What the move carries besides the engram: the attachments it
        // references or claims. Filled in the cross-domain branch and acted on
        // once every store lock that branch takes has been released. Beside it,
        // the attachments the plan could not carry, which ride out in the
        // receipt so a caller who never sees the daemon's trace still learns
        // what stayed behind.
        let mut carried: Vec<AttachmentCarry> = Vec::new();
        let mut attachment_warnings: Vec<String> = Vec::new();
        // The checksum of the text that landed at the destination, for the
        // change this move announces at its tail.
        let moved_checksum: String;

        // Set once the move has committed (see [`TellMoved`]).
        let mut told: TellMoved;
        if cross {
            // Index the content into the destination source, then remove the
            // source.
            let mut content = moved_text.clone().unwrap_or_else(|| original.clone());
            // Resolved before the write, since an attachment that has to be
            // renamed at the destination changes the very text being written:
            // the engram lands already pointing at the name its file took.
            (carried, attachment_warnings) = self
                .plan_attachment_carry(&src, &dest_domain, &content)
                .await;
            let renames: BTreeMap<String, String> = carried
                .iter()
                .filter(|carry| carry.to != carry.from)
                .map(|carry| (carry.from.clone(), carry.to.clone()))
                .collect();
            if !renames.is_empty() {
                content = rewrite_carried_refs(&content, &renames);
            }
            // The moved engram's own text passes through the same funnel
            // every other write's final text does: a cross-domain link it
            // carries, spelled with an alias or a non-canonical local name,
            // is written with the canonical one instead. The move's own
            // relink pass above only follows THIS engram's address; a link to
            // some other domain is untouched by it and would otherwise cross
            // domains still spelled however it was written.
            let (content, _) = self.normalize_domain_spellings(&content, &hidden).await;
            moved_checksum = sha256_hex(content.as_bytes());
            match &dest_source {
                ContentSource::File { root } => {
                    let dest_abs = join_rel(root, &dest_rel);
                    write_file(&dest_abs, &content)?;
                    let indexed = async {
                        let store = self.store.lock().await;
                        let dest_id = store
                            .upsert_domain(
                                &dest_domain,
                                Some(&root.to_string_lossy()),
                                DomainKind::File,
                            )
                            .await?;
                        self.reindex_file(&*store, dest_id, root, &dest_rel).await
                    }
                    .await;
                    // The destination's host lock moved meanwhile: the file
                    // written there goes again, and the source stays.
                    if let Err(e) = indexed {
                        return Err(self.refuse_file_change(
                            e,
                            &dest_domain,
                            vec![(
                                dest_abs.clone(),
                                None,
                                Some(crystalline_core::to_lf(&content).into_owned().into_bytes()),
                            )],
                        ));
                    }
                }
                ContentSource::Virtual => {
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(&dest_domain, None, DomainKind::Virtual)
                        .await?;
                    let stamp = virtual_stamp(&content);
                    self.index_markdown(&*store, dest_id, &dest_rel, &content, stamp, None, true)
                        .await?;
                }
            }
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                if let Err(e) = std::fs::remove_file(&src_abs) {
                    tracing::warn!(
                        "could not remove moved source {}: {e}; leaving it in place",
                        src_abs.display()
                    );
                }
            }
            let store = self.store.lock().await;
            // One transaction for the source row, as the index half of a
            // delete has: the row, its children and the unbinding of every
            // reference that named it land together, behind the source's
            // host-lock fence.
            store.begin().await?;
            if let ContentSource::File { root } = &src_source
                && let Err(e) = self.file_row_fence(&*store, src.domain_id, root).await
            {
                let _ = store.rollback().await;
                drop(store);
                // The source's lock moved after the destination landed: the
                // source file comes back, and the engram is in both places
                // until one of them is removed.
                return Err(self.refuse_file_change(
                    e,
                    &src.domain,
                    vec![(
                        join_rel(root, &src.path),
                        Some(original.clone().into_bytes()),
                        None,
                    )],
                ));
            }
            if let Err(e) = store.delete_engram(src.domain_id, &src.path).await {
                let _ = store.rollback().await;
                return Err(e.into());
            }
            store.commit().await?;
            // The engram has left the source: whatever fails from here, the
            // room and later joins are told where it went.
            told = TellMoved(closing.take());
        } else {
            // Same-domain rename: move the file when file-backed, then give the
            // row its new path and permalink in place, keeping its id. A move
            // that did not change the text needs no reparse; one that did is
            // reindexed from what was written, so the row reads the file.
            //
            // When the move DID change the text (a permalink line, or a
            // relinked self-reference), that text passes through the same
            // funnel every other write's final text does: a cross-domain
            // link it carries to some other domain, spelled with an alias or
            // a non-canonical local name, is written with the canonical one
            // instead. Byte-identical carries (`None`) are untouched, so an
            // ordinary rename never rewrites content the move itself did not
            // touch.
            let moved_text = match moved_text {
                Some(text) => Some(self.normalize_domain_spellings(&text, &hidden).await.0),
                None => None,
            };
            // A plain rename carries `original` across byte for byte; a
            // readdressing one lands `moved_text`.
            moved_checksum = sha256_hex(
                moved_text
                    .as_deref()
                    .map_or(original.as_bytes(), str::as_bytes),
            );
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                let dest_abs = join_rel(root, &dest_rel);
                // A letter-case rename on a case-insensitive disk renames the
                // file first (atomic, and it stays on disk under one name or
                // the other if the write fails), then writes the new text.
                let case_only = !cross
                    && !in_place
                    && is_case_only_rename(&src_abs, &dest_abs, &src.path, &dest_rel);
                if case_only {
                    std::fs::rename(&src_abs, &dest_abs).map_err(|source| EngineError::Io {
                        path: dest_abs.display().to_string(),
                        source,
                    })?;
                }
                match &moved_text {
                    Some(text) => write_file(&dest_abs, text)?,
                    None => write_bytes(&dest_abs, original.as_bytes())?,
                }
                if !in_place && !case_only {
                    std::fs::remove_file(&src_abs).map_err(|source| EngineError::Io {
                        path: src_abs.display().to_string(),
                        source,
                    })?;
                }
            }
            let store = self.store.lock().await;
            // The readdress and the reindex of a file domain commit behind
            // its host-lock fence; a refusal puts both files back.
            let put_back_files = |root: &Path| {
                let written = moved_text
                    .as_deref()
                    .map_or(original.as_bytes().to_vec(), |t| {
                        crystalline_core::to_lf(t).into_owned().into_bytes()
                    });
                let (src_abs, dest_abs) = (join_rel(root, &src.path), join_rel(root, &dest_rel));
                if src_abs == dest_abs {
                    vec![(src_abs, Some(original.clone().into_bytes()), Some(written))]
                } else {
                    vec![
                        (dest_abs, None, Some(written)),
                        (src_abs, Some(original.clone().into_bytes()), None),
                    ]
                }
            };
            let readdressed_row = async {
                store.begin().await?;
                let done = async {
                    if let ContentSource::File { root } = &src_source {
                        self.file_row_fence(&*store, src.domain_id, root).await?;
                    }
                    store
                        .readdress_engram(src.domain_id, &src.path, &dest_rel, &new_permalink)
                        .await?;
                    Ok::<(), EngineError>(())
                }
                .await;
                match done {
                    Ok(()) => Ok(store.commit().await?),
                    Err(e) => {
                        let _ = store.rollback().await;
                        Err(e)
                    }
                }
            }
            .await;
            if let Err(e) = readdressed_row {
                return Err(match &src_source {
                    ContentSource::File { root } => {
                        self.refuse_file_change(e, &src.domain, put_back_files(root))
                    }
                    ContentSource::Virtual => e,
                });
            }
            // The row answers to its new address now: whatever fails from
            // here, the room and later joins are told where it went.
            told = TellMoved(closing.take());
            if let Some(text) = &moved_text {
                match &src_source {
                    ContentSource::File { root } => {
                        if let Err(e) = self
                            .reindex_file(&*store, src.domain_id, root, &dest_rel)
                            .await
                        {
                            return Err(self.refuse_file_change(
                                e,
                                &src.domain,
                                put_back_files(root),
                            ));
                        }
                    }
                    ContentSource::Virtual => {
                        self.index_markdown(
                            &*store,
                            src.domain_id,
                            &dest_rel,
                            text,
                            virtual_stamp(text),
                            None,
                            true,
                        )
                        .await?;
                    }
                }
            } else if readdressed {
                // A reference somebody wrote ahead of time to the new address
                // binds now rather than at the next sync. The reindex arm above
                // resolves inside its own transaction.
                store.resolve_pending_relations(src.domain_id).await?;
                store.resolve_pending_links(src.domain_id).await?;
            }
        }

        drop(_lock);
        // The engram has moved: the room's sockets learn its new address now,
        // and the fence lifts. A way out before the commit dropped the guard
        // instead, which told the room `internal`; one after it leaves the
        // telling to `told`'s drop.
        if let Some(closing) = told.0.take() {
            closing.moved().await;
        }

        // The attachments follow the engram, now that the engram itself has
        // landed and the branch above has released its store lock. A
        // same-domain move carries nothing: an `assets/` reference is
        // domain-root relative, so a rename inside one domain leaves every one
        // of them valid as written.
        if !carried.is_empty() {
            self.carry_attachments(&src.domain, &dest_domain, &carried)
                .await;
        }

        // Rewrite every reference that followed the engram. The referencing
        // engrams were not authored by whoever asked for the move, so their
        // refreshed `generated.by` records Crystalline itself (or the
        // configured `identity.actor`), not the moving client. A referencing
        // engram that cannot be rewritten is logged and skipped rather than
        // failing the call: the move is committed by this line, and an error
        // now would report a move that happened as one that did not. The
        // receipt names it in `links_failed`, as a reflection move does.
        let linker_actor = self.actor(None);
        // The moved engram's own rebase, deferred above because a room is
        // open over it: composed into the room when it saves, else written to
        // the file at its new path. Its own text, so the mover is its author.
        if defer_rebase && let Some(moved) = &relocation {
            let own = MoveLinker {
                domain: p.domain.clone(),
                domain_id: src.domain_id,
                path: dest_rel.clone(),
            };
            if let Err(e) = self
                .relink_engram(&own, &spec, Some(moved), &self.actor(client), &hidden)
                .await
            {
                tracing::warn!(
                    domain = p.domain.as_str(),
                    path = dest_rel.as_str(),
                    "the move could not rebase the moved engram's own relative links: {e}"
                );
                note_link_failure(&mut failed, &p.domain, &dest_rel, &e);
            }
        }
        let mut rewritten: Vec<Value> = Vec::new();
        let mut references = 0usize;
        for linker in &linkers {
            if read_only_linkers.contains(&linker.domain) {
                failed.push(json!({
                    "domain": linker.domain,
                    "path": linker.path,
                    "reason": LINKER_READ_ONLY,
                }));
                continue;
            }
            if let Some(e) = paused.get(&linker.domain) {
                tracing::warn!(
                    domain = linker.domain.as_str(),
                    path = linker.path.as_str(),
                    "the move could not rewrite the references in this engram: {e}"
                );
                note_link_failure(&mut failed, &linker.domain, &linker.path, e);
                continue;
            }
            let mut notes = None;
            let relinked = self
                .relink_engram_noting(
                    linker,
                    &spec,
                    relocation.as_ref(),
                    &linker_actor,
                    &hidden,
                    &mut notes,
                )
                .await;
            if let Some(reason) = notes {
                failed.push(json!({
                    "domain": linker.domain,
                    "path": linker.path,
                    "reason": reason,
                }));
            }
            match relinked {
                Ok(Some(relinked)) => {
                    references += relinked.references;
                    // The path rides along so a headless run's share can
                    // carry the rewritten linker (the MCP dispatch notes it).
                    let mut entry = json!({
                        "domain": linker.domain,
                        "permalink": relinked.permalink,
                        "path": linker.path,
                    });
                    // Written into a room somebody has open, which saves it.
                    if relinked.live {
                        entry["landed"] = json!("live");
                    }
                    rewritten.push(entry);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        domain = linker.domain.as_str(),
                        path = linker.path.as_str(),
                        "the move could not rewrite the references in this engram: {e}"
                    );
                    note_link_failure(&mut failed, &linker.domain, &linker.path, &e);
                }
            }
        }

        // When either end of the move is a virtual domain, a MANIFEST engram
        // may have moved into or out of it, so refresh the routing cache. Every
        // store lock taken above is released by here.
        if matches!(src_source, ContentSource::Virtual)
            || matches!(dest_source, ContentSource::Virtual)
        {
            self.refresh_routing_cache().await;
        }
        // A move empties one folder and fills another, so both ends need their
        // index files back in step; a same-domain move refreshes once.
        // The linkers it rewrote changed only their bodies, never a title.
        if cross {
            self.refresh_index_files(&p.domain, crate::index_files::Touched::path(&src.path))
                .await;
            self.refresh_index_files(&dest_domain, crate::index_files::Touched::path(&dest_rel))
                .await;
        } else {
            let both = vec![src.path.clone(), dest_rel.clone()];
            self.refresh_index_files(&p.domain, crate::index_files::Touched::Paths(both))
                .await;
        }

        // The address the engram answers to at its destination, asked rather
        // than assumed, exactly as `save_engram` asks: a permalink that was
        // derived from the path follows the move (the store's rename says so
        // in as many words), so a receipt repeating the one it went in with
        // would name a permalink that no longer resolves on the very calls
        // that changed it. The move is committed by this line, so the asking is
        // tolerant: a missing row and a failing lookup alike fall back to the
        // name it went in with rather than failing a done move - see
        // [`receipt_permalink`].
        let dest_permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(&dest_domain, Some(&dest_rel), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == dest_rel)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, new_permalink.clone())
        };

        // A subscriber may see only one of two domains, so a cross-domain
        // move is a `deleted` in the source and an `added` in the
        // destination; a rename inside one domain, a permalink-only rename in
        // place included, is one `moved` with `from`. The linkers the move
        // rewrote announced themselves in `relink_engram`.
        let label = change_label(scope);
        if cross {
            self.announce(Change::Engram(EngramChanged {
                domain: p.domain.clone(),
                permalink: src.permalink.clone(),
                path: src.path.clone(),
                kind: ChangeKind::Deleted,
                from: None,
                checksum: None,
                actor: label.clone(),
                draft_of: None,
                audience: None,
            }));
            self.announce(Change::Engram(EngramChanged {
                domain: dest_domain.clone(),
                permalink: dest_permalink.clone(),
                path: dest_rel.clone(),
                kind: ChangeKind::Added,
                from: None,
                checksum: Some(moved_checksum),
                actor: label,
                draft_of: None,
                audience: None,
            }));
        } else {
            self.announce(Change::Engram(EngramChanged {
                domain: p.domain.clone(),
                permalink: dest_permalink.clone(),
                path: dest_rel.clone(),
                kind: ChangeKind::Moved,
                from: Some(MovedFrom {
                    path: src.path.clone(),
                    permalink: src.permalink.clone(),
                }),
                checksum: Some(moved_checksum),
                actor: label,
                draft_of: None,
                audience: None,
            }));
        }
        // Moving the last engram out of `reflections/`, or a file onto the
        // MANIFEST, can move a Reflection's mode in either domain.
        self.heal_reflection_rows_after(&p.domain, &src.path).await;
        if cross || src.path != dest_rel {
            self.heal_reflection_rows_after(&dest_domain, &dest_rel)
                .await;
        }
        if let Some(before) = switch_before {
            self.announce_switch_if_moved(&dest_domain, before, change_label(scope));
        }
        // The new address may be what a link in another domain has been
        // waiting for. A draft move returned above, through `move_within`.
        if readdressed {
            self.bind_references_to(&dest_domain, &dest_permalink, &src.title)
                .await;
        }

        // `links_rewritten` counts engrams, as it always has; the references
        // inside them are `references_rewritten`, and `rewritten` names the
        // engrams so a caller can re-read what changed under it.
        // `links_failed` appears only when a linker could not be rewritten.
        let mut receipt = json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": dest_domain, "permalink": dest_permalink, "path": dest_rel },
            "cross_domain": cross,
            "links_rewritten": rewritten.len(),
            "references_rewritten": references,
            "rewritten": rewritten,
            "attachment_warnings": attachment_warnings,
        });
        if !failed.is_empty() {
            receipt["links_failed"] = json!(failed);
        }
        // A move into another domain names the moved engram's own links that
        // do not resolve where it landed, present only when there is one.
        if let Some(cross_links) = &cross_links {
            let dead = self
                .dead_links(&dest_domain, &dest_permalink, &cross_links.unresolved)
                .await;
            if !dead.is_empty() {
                receipt["links_dead"] = json!(dead);
            }
        }
        Ok(receipt)
    }

    /// The permalink a moved engram answers to at `dest_rel`.
    ///
    /// Omitted, the permalink follows the move only when it was **in step**
    /// with the old path - equal to the slug the old path derives, which is
    /// also what an engram with no `permalink:` line answers to - and stays
    /// otherwise, since a permalink that differs from its path was either
    /// chosen on purpose or drifted, and only the caller knows which. `"path"`
    /// takes the destination path's own, `"keep"` keeps the current one
    /// whatever it is, and any other value is validated as a permalink and
    /// taken as given. The two keywords are why neither can be asked for as a
    /// literal permalink; an engram that should answer to `path` is written
    /// to `path.md` and moved with `"path"`.
    fn moved_permalink(
        src: &EngramDescriptor,
        dest_rel: &str,
        asked: Option<&str>,
    ) -> Result<String> {
        let from_path = crystalline_core::path_permalink(dest_rel);
        let chosen = match asked.map(str::trim) {
            None | Some("") => {
                if src.permalink == crystalline_core::path_permalink(&src.path) {
                    from_path
                } else {
                    src.permalink.clone()
                }
            }
            Some("path") => from_path,
            Some("keep") => src.permalink.clone(),
            Some(named) => {
                crystalline_core::validate_permalink(named).map_err(EngineError::Invalid)?;
                named.to_string()
            }
        };
        if chosen.is_empty() {
            return Err(EngineError::Invalid(format!(
                "the destination '{dest_rel}' does not slugify to a permalink; use a path with \
                 letters or digits, or pass a permalink"
            )));
        }
        Ok(chosen)
    }

    /// The text a moved engram lands with, or `None` when the move leaves its
    /// bytes exactly as they are.
    ///
    /// Two things can change it. The `permalink:` line, rewritten surgically
    /// (or added) whenever what the text would answer to at `dest_rel` - its
    /// own `permalink:` line, or the destination path's slug when it has none -
    /// is not `new_permalink`: that is what makes `"keep"` hold for an engram
    /// whose permalink was only ever path-derived, and what makes the new
    /// address survive the next sync. When `relink` is given, the engram's
    /// references to itself, which follow it like everybody else's. And, when
    /// `relocation` is given, its relative links, rebased onto its new folder
    /// so each lands on the file it landed on before. When `cross` is given
    /// (a move into another domain), its links that bind by path, which
    /// would be read in the destination, name what they reached in the
    /// source instead (see [`Engine::cross_links`]); the second value is the
    /// `links_failed` reason for the ones that could not. Any change touches
    /// `generated` with the mover; `recorded_at` is never touched, because the
    /// knowledge was not recorded again.
    fn readdressed_text(
        original: &str,
        dest_rel: &str,
        new_permalink: &str,
        relink: Option<&Relink<'_>>,
        relocation: Option<&Relocation<'_>>,
        cross: Option<&CrossLinks>,
        mover: &str,
    ) -> Result<(Option<String>, Option<String>)> {
        let engram = parse_engram(original).map_err(|e| {
            EngineError::Invalid(format!(
                "the engram does not parse ({e}); fix it before moving it"
            ))
        })?;
        let implied = engram
            .frontmatter
            .permalink
            .filter(|permalink| !permalink.is_empty())
            .unwrap_or_else(|| crystalline_core::path_permalink(dest_rel));
        let mut text = original.to_string();
        if implied != new_permalink {
            text = set_frontmatter_field(&text, "permalink", new_permalink);
        }
        // Before the self-references follow: a link that reached another
        // engram of the source names it by its address from here on.
        if let Some(cross) = cross {
            text =
                crystalline_core::relink::readdress_wikilinks(&text, &|domain, target, shown| {
                    cross
                        .readdressed
                        .get(&(domain.map(str::to_string), target.to_string()))
                        .map(|to| to.written(shown))
                })
                .0;
        }
        if let Some(spec) = relink {
            text =
                crystalline_core::relink::relink(&text, spec.from_domain, spec.to_domain, spec).0;
        }
        if let Some(moved) = relocation {
            text = crystalline_core::relink::rebase_relative_links(&text, moved.from, moved.to).0;
            // A wikilink spelled as a path is read from the domain root, so
            // only one naming the engram itself has to follow it.
            text = crystalline_core::relink::respell_path_wikilinks(&text, moved.from, moved.to).0;
        }
        let mut notes = None;
        if let Some(cross) = cross {
            let whole = crystalline_core::relink::readdress_relative_links(&text, &|target| {
                cross.relative.get(target).cloned()
            });
            // After the self-references followed, so a bare one can no longer
            // be read as a self-reference of the domain it left.
            text =
                crystalline_core::relink::readdress_wikilinks(&whole.text, &|domain, target, _| {
                    let domain = domain?;
                    cross
                        .collapsed
                        .contains(&(domain.to_string(), target.to_string()))
                        .then(|| target.to_string())
                })
                .0;
            notes = cross_link_reason(&cross.stranded, &whole.unwritable, &whole.dropped);
        }
        let text = (text != original).then(|| touch_generated(&text, mover, None, now_offset()));
        Ok((text, notes))
    }

    /// What a move of the engram at `src_path` in `src_domain` to `dest_rel`
    /// in `dest_domain` does with its own links that bind by path, read from
    /// `text`. Once it lands, those would be read in the destination, where
    /// the same path may be another engram. So each relative link that
    /// reaches into the source domain names it: `[[src:permalink|text]]`
    /// where an engram stands at the path, `[[src:path.md|text]]` where none
    /// does (it stays unresolved, as it was). A link to the engram itself is
    /// rebased to follow it, a link to a folder's `index.md` or `log.md` stays
    /// (every folder has its own), and a link that climbs out of the source
    /// domain stays as written, named in `stranded` when it would land on an
    /// engram in the destination. A bare wikilink spelled as a source
    /// engram's path gets the source's prefix the same way, and so does every
    /// other bare wikilink and relation that resolved in the source (issue
    /// 152): a bare link is read in the domain it sits in, so once the engram
    /// lands it would look for its target in the destination. One that reached
    /// the engram itself, or nothing, stays bare. A prefix that names the
    /// destination is dropped once the engram lands there.
    async fn cross_links(
        &self,
        src_domain: &str,
        src_path: &str,
        dest_domain: &str,
        dest_rel: &str,
        text: &str,
    ) -> Result<CrossLinks> {
        use crystalline_core::relink::NoteLinkTo;
        let mut out = CrossLinks::default();
        let Ok(engram) = parse_engram(text) else {
            return Ok(out);
        };
        let names = self.name_table_now().await;
        let targets: Vec<&LinkTarget> = engram
            .links
            .iter()
            .map(|l| &l.target)
            .chain(engram.relations.iter().map(|r| &r.target))
            .collect();
        for target in targets {
            match target.form {
                LinkForm::Relative => {
                    if out.relative.contains_key(&target.target)
                        || out.stranded.iter().any(|s| s == &target.target)
                    {
                        continue;
                    }
                    let landed = crystalline_core::reflection::links::resolve_in_domain(
                        src_path,
                        &target.target,
                    );
                    let to = match landed {
                        Some(path) if path == src_path => Some(NoteLinkTo::Target(
                            crystalline_core::reflection::relative_to(dest_rel, dest_rel),
                        )),
                        Some(path) if crystalline_core::is_reserved_path(&path) => None,
                        Some(path) => Some(NoteLinkTo::Wikilink(
                            match self.descriptor_at(src_domain, &path).await? {
                                Some(found) => format!("{src_domain}:{}", found.permalink),
                                None => format!("{src_domain}:{path}"),
                            },
                        )),
                        None => {
                            let there = crystalline_core::reflection::links::resolve_in_domain(
                                dest_rel,
                                &target.target,
                            );
                            if let Some(there) = there
                                && self.descriptor_at(dest_domain, &there).await?.is_some()
                            {
                                out.stranded.push(target.target.clone());
                            }
                            None
                        }
                    };
                    if let Some(to) = to {
                        out.relative.insert(target.target.clone(), to);
                    }
                }
                LinkForm::Wiki => {
                    let key = (target.domain.clone(), target.target.clone());
                    let whole_key = target
                        .whole
                        .as_ref()
                        .map(|whole| (target.domain.clone(), whole.clone()));
                    if out.readdressed.contains_key(&key)
                        || whole_key
                            .as_ref()
                            .is_some_and(|k| out.readdressed.contains_key(k))
                    {
                        continue;
                    }
                    // A title that holds a `#` (`[[C# Basics]]`) is read whole
                    // first, as the resolver reads it, and keyed by the whole
                    // text so the rewrite replaces it fragment and all.
                    let mut key = key;
                    let to = match &target.domain {
                        None if target.target.to_ascii_lowercase().ends_with(".md") => {
                            // Its own path follows it, and is read in the
                            // destination from there on.
                            if target.target == src_path {
                                Some(CrossTarget {
                                    to: dest_rel.to_string(),
                                    words: None,
                                })
                            } else {
                                // Read in the destination from now on, so it
                                // names the source domain: by permalink where
                                // an engram stands at the path, by the path
                                // where none does (it stays unresolved, as it
                                // was).
                                let address =
                                    match self.descriptor_at(src_domain, &target.target).await? {
                                        Some(found) => format!("{src_domain}:{}", found.permalink),
                                        None => format!("{src_domain}:{}", target.target),
                                    };
                                Some(CrossTarget {
                                    to: address,
                                    words: None,
                                })
                            }
                        }
                        // A bare link resolves in the domain it is read in,
                        // so one that reached an engram of the source names
                        // it by its address. One that reached the engram
                        // itself is left to the self-references, and one
                        // that reached nothing stays as written.
                        None => {
                            let whole = match &target.whole {
                                Some(whole) => {
                                    self.cross_title(src_domain, src_path, whole).await?
                                }
                                None => TitleHit::Nothing,
                            };
                            match whole {
                                TitleHit::Other(to) => {
                                    if let Some(whole_key) = whole_key {
                                        key = whole_key;
                                    }
                                    Some(to)
                                }
                                // The engram's own title: left to the
                                // self-references, never read split.
                                TitleHit::Itself => None,
                                TitleHit::Nothing => {
                                    self.cross_target(src_domain, src_path, &target.target)
                                        .await?
                                }
                            }
                        }
                        Some(prefix) => match names.resolve(prefix) {
                            // A prefix that names the destination: written
                            // bare once the engram lands there, unless the
                            // bare text would read as a prefix itself.
                            Some(local) if local == dest_domain => {
                                if LinkTarget::parse(&target.target).domain.is_none() {
                                    out.collapsed
                                        .insert((prefix.clone(), target.target.clone()));
                                }
                                None
                            }
                            Some(_) => None,
                            // A prefix no domain answers to is no prefix: the
                            // resolver reads the whole text at home (a title
                            // such as `Log: Weekly`), so it is a bare link.
                            None => self.cross_target(src_domain, src_path, &target.raw).await?,
                        },
                    };
                    if let Some(to) = to {
                        out.readdressed.insert(key, to);
                    }
                }
            }
        }
        Ok(out)
    }

    /// What the whole text of a bare wikilink whose title holds a `#`
    /// ([`LinkTarget::whole`]) reached in `src_domain` as a title, written as
    /// [`Engine::cross_target`] writes it.
    async fn cross_title(&self, src_domain: &str, src_path: &str, whole: &str) -> Result<TitleHit> {
        let found = {
            let store = self.store.lock().await;
            store.find_engram(src_domain, whole).await?
        };
        let Some(found) = found.filter(|f| f.title.to_lowercase() == whole.to_lowercase()) else {
            return Ok(TitleHit::Nothing);
        };
        if found.path == src_path {
            return Ok(TitleHit::Itself);
        }
        Ok(TitleHit::Other(CrossTarget {
            to: format!("{src_domain}:{}", found.permalink),
            words: Some(whole.to_string()),
        }))
    }

    /// The whole texts with a `#` of the wikilinks in `text`, read in
    /// `read_in`, that the domain the moved engram left holds as the title of
    /// another engram (`[[C# Basics]]` beside a moved `C`), for
    /// [`Relink::shadowed`]: the resolver reads them whole, so they point
    /// there and a relink must leave them alone.
    async fn shadowed_titles(
        &self,
        text: &str,
        read_in: &str,
        spec: &Relink<'_>,
    ) -> Result<Vec<String>> {
        let Ok(engram) = parse_engram(text) else {
            return Ok(Vec::new());
        };
        let moved_title = spec.title.trim().to_lowercase();
        let mut asked: Vec<&String> = Vec::new();
        for target in engram
            .links
            .iter()
            .map(|l| &l.target)
            .chain(engram.relations.iter().map(|r| &r.target))
        {
            let Some(whole) = &target.whole else {
                continue;
            };
            let read_there = match &target.domain {
                Some(domain) => domain == spec.from_domain,
                None => read_in == spec.from_domain,
            };
            let lower = whole.to_lowercase();
            if read_there
                && lower != moved_title
                && !asked.iter().any(|a| a.to_lowercase() == lower)
            {
                asked.push(whole);
            }
        }
        if asked.is_empty() {
            return Ok(Vec::new());
        }
        // Every text asked under one store lock.
        let store = self.store.lock().await;
        let mut out: Vec<String> = Vec::new();
        for whole in asked {
            let lower = whole.to_lowercase();
            let found = store.find_engram(spec.from_domain, whole).await?;
            if found.is_some_and(|f| f.title.to_lowercase() == lower) {
                out.push(whole.clone());
            }
        }
        Ok(out)
    }

    /// What a bare wikilink `target` of the engram at `src_path` reached in
    /// `src_domain`, as the address it is written with once the engram leaves
    /// the domain; `None` when it reached the engram itself or nothing.
    ///
    /// Read the way the index resolves a reference: the permalink first, then
    /// the title in any letter case, then the domain-relative path. A link by
    /// title names the permalink and keeps its words as the display text.
    async fn cross_target(
        &self,
        src_domain: &str,
        src_path: &str,
        target: &str,
    ) -> Result<Option<CrossTarget>> {
        let found = {
            let store = self.store.lock().await;
            store.find_engram(src_domain, target).await?
        };
        let (found, by_title) = match found {
            Some(found) => {
                let by_title = found.permalink != target;
                (found, by_title)
            }
            None => match self.descriptor_at(src_domain, target).await? {
                Some(found) => (found, false),
                None => return Ok(None),
            },
        };
        if found.path == src_path {
            return Ok(None);
        }
        Ok(Some(CrossTarget {
            to: format!("{src_domain}:{}", found.permalink),
            words: by_title.then(|| target.to_string()),
        }))
    }

    /// The moved engram's references the index cannot resolve now that it
    /// has landed at `permalink` in `domain`, as the receipt's `links_dead`
    /// names them: `{ link, reason }`, where the reason is
    /// `unresolved_before` for a link that resolved nowhere before the move
    /// either and `unresolved_after` for one the move left dead (a guard: a
    /// move is meant to keep every link it found resolving). Asked of the
    /// index, the resolver's own verdict, after every rewrite has landed. A
    /// failing lookup names nothing rather than failing a done move.
    async fn dead_links(
        &self,
        domain: &str,
        permalink: &str,
        unresolved_before: &HashSet<String>,
    ) -> Vec<Value> {
        let store = self.store.lock().await;
        let refs = match store.lookup_id(domain, permalink).await {
            Ok(Some(id)) => store.outbound_refs(id, None).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut seen = HashSet::new();
        refs.into_iter()
            .filter(|r| !r.resolved)
            .filter_map(|r| {
                // Named with its `#heading`, as it is written.
                let named = r.to_whole.as_deref().unwrap_or(&r.to_target);
                let link = match &r.to_domain {
                    Some(prefix) => format!("[[{prefix}:{named}]]"),
                    None => format!("[[{named}]]"),
                };
                let reason = if unresolved_before.contains(named) {
                    "unresolved_before"
                } else {
                    "unresolved_after"
                };
                seen.insert(link.clone())
                    .then(|| json!({ "link": link, "reason": reason }))
            })
            .collect()
    }

    /// Refuse a move onto a permalink another engram in the destination domain
    /// already answers to, naming the holder, before anything is written.
    ///
    /// Asked by permalink alone: [`ReadStore::find_engram`] also matches a title,
    /// and an engram titled like the new permalink does not hold the address.
    /// The moved engram itself is not a holder on a same-domain move, since
    /// its row is the one being readdressed.
    async fn refuse_permalink_held(
        &self,
        dest_domain: &str,
        new_permalink: &str,
        src: &EngramDescriptor,
        cross: bool,
    ) -> Result<()> {
        let holder = {
            let store = self.store.lock().await;
            store
                .find_engram(dest_domain, new_permalink)
                .await?
                .filter(|found| found.permalink == new_permalink)
                .filter(|found| cross || found.path != src.path)
        };
        match holder {
            Some(holder) => Err(EngineError::Conflict(format!(
                "permalink '{new_permalink}' is already held by '{}' in domain '{dest_domain}'; \
                 pick another permalink, or move that engram first",
                holder.path
            ))),
            None => Ok(()),
        }
    }

    /// End the room over `doc` for a move to `to`, after its last save,
    /// and fence the document while the returned guard stands (see
    /// [`CollabSessions::close_for_move`]). The move calls
    /// [`MoveClose::moved`] once it has committed; dropped on any other way
    /// out, the guard tells the room `internal`. `None` on an engine with no
    /// rooms: nothing to close and nobody to fence out.
    ///
    /// Runs after every screen that can refuse the move, so a refused move
    /// leaves the room open, and before the file locks, which a room's save
    /// takes.
    ///
    /// [`CollabSessions::close_for_move`]: crate::collab::session::CollabSessions::close_for_move
    pub(super) async fn close_room_for_move(
        &self,
        domain: &str,
        doc: RoomDoc,
        overlay: Option<&str>,
        to: MovedTo,
    ) -> Option<MoveClose> {
        let rooms = self.collab_rooms()?;
        Some(rooms.close_for_move(domain, &doc, overlay, to).await)
    }

    /// Refuse while a room is open over any of the reflection pages
    /// `paths` (inside `reflections/`): a reflection room saves straight to
    /// its file, so a delete, or a rewrite of its links, would race its next
    /// save. A move of the page itself does not come here; it closes the
    /// room instead (`Engine::close_room_for_move`).
    pub(super) async fn refuse_under_live_pages(
        &self,
        domain: &str,
        paths: &[String],
    ) -> Result<()> {
        let Some(rooms) = self.collab_rooms() else {
            return Ok(());
        };
        for path in paths {
            let doc = RoomDoc::Reflection(path.clone());
            if rooms.has_live_doc(domain, &doc, None).await {
                return Err(EngineError::Conflict(format!(
                    "somebody is editing '{path}'; try again once the page is closed"
                )));
            }
        }
        Ok(())
    }

    /// The engrams whose references a move rewrites: every base engram with a
    /// relation or link that points at `src`, plus every one whose content
    /// holds its `crystalline://` URL (no edge table records those), in
    /// domains this caller may see, each once, `src` itself excluded - its own
    /// references travel with its text.
    pub(super) async fn move_linkers(
        &self,
        src: &EngramDescriptor,
        hidden: &HashSet<String>,
    ) -> Result<Vec<MoveLinker>> {
        let url = format!(
            "{}{}/{}",
            crystalline_core::address::SCHEME,
            src.domain,
            src.permalink
        );
        let (refs, mentions) = {
            let store = self.store.lock().await;
            (
                store
                    .inbound_refs(src.id, src.domain_id, &src.permalink, &src.title)
                    .await?,
                store.engrams_mentioning(&url).await?,
            )
        };
        let candidates = refs
            .into_iter()
            .map(|r| (r.src_domain, r.src_domain_id, r.src_path))
            .chain(
                mentions
                    .into_iter()
                    .map(|m| (m.domain, m.domain_id, m.path)),
            );
        let mut seen: HashSet<(i64, String)> = HashSet::new();
        let mut linkers = Vec::new();
        for (domain, domain_id, path) in candidates {
            if hidden.contains(&domain)
                || (domain_id == src.domain_id && path == src.path)
                // A read-only domain's link is left as it was, like a hidden
                // domain's (decision D7); so is an archived repository's.
                || self.writes_refused(&domain).await
            {
                continue;
            }
            if seen.insert((domain_id.0, path.clone())) {
                linkers.push(MoveLinker {
                    domain,
                    domain_id,
                    path,
                });
            }
        }
        Ok(linkers)
    }

    /// Rewrite the references to a moved engram inside one referencing engram,
    /// in its source of truth, then reindex it. Answers how many references
    /// changed and the engram's permalink, or `None` when its text held none
    /// after all - a content hit inside code, or a longer permalink.
    ///
    /// Written the way an edit writes, because an edit of the same engram may
    /// run beside the move. A file is read, rewritten and reindexed under its
    /// write lock, which every other writer of it takes too, so neither change
    /// is lost. A virtual row is written by the store's compare and swap, and
    /// a refused swap is applied again to the row stored now, as an unguarded
    /// edit is.
    ///
    /// While a co-editing room that will save is open over the linker, the
    /// room's text IS the engram, so the rewrite composes into it, the way an
    /// edit's live arm does: computed from the live text, it never collides
    /// with what a person is typing, and the room's own save writes it down
    /// and announces it. Neither the file nor the row is written behind that
    /// room, and the receipt marks the linker `landed: live`.
    ///
    /// A room that will not save what it is given (a conflict, a failed save,
    /// or one whose last connection left, whose final save has run or is
    /// running) gets nothing: the rewrite goes to the file or the row, and the
    /// room meets it as an external change on its next save, by its compare
    /// and swap and a three-way merge. The same holds when the room keeps
    /// moving under the compose because somebody is typing.
    ///
    /// Lock order: the room first, holding nothing else, then the file lock,
    /// then the store lock, as `edit_engram` takes them and as a room's save
    /// takes them. The move holds no other file lock and no store lock while
    /// it waits here, so an edit of this engram and the move cannot wait on
    /// each other.
    pub(super) async fn relink_engram(
        &self,
        linker: &MoveLinker,
        spec: &Relink<'_>,
        relocation: Option<&Relocation<'_>>,
        actor: &str,
        hidden: &HashSet<String>,
    ) -> Result<Option<Relinked>> {
        let mut notes = None;
        self.relink_engram_noting(linker, spec, relocation, actor, hidden, &mut notes)
            .await
    }

    /// [`Engine::relink_engram`], and in `notes` the `links_failed` reason
    /// for the links to the moved engram it could not follow it with (a move
    /// into another domain gives a relative link an address, which a link
    /// text holding `[[` or `]]` cannot carry).
    pub(super) async fn relink_engram_noting(
        &self,
        linker: &MoveLinker,
        spec: &Relink<'_>,
        relocation: Option<&Relocation<'_>>,
        actor: &str,
        hidden: &HashSet<String>,
        notes: &mut Option<String>,
    ) -> Result<Option<Relinked>> {
        // The live arm. The room is keyed on the linker's permalink, which
        // the index knows; asked before any lock is taken, and only when a
        // room over one of this domain's engrams is open at all.
        if let Some(rooms) = self.collab_rooms()
            && rooms.has_engram_rooms_in(&linker.domain).await
            && let Some(desc) = self.descriptor_at(&linker.domain, &linker.path).await?
        {
            for attempt in 0..super::edit::UNGUARDED_EDIT_ATTEMPTS {
                let Some(live) = rooms.live_text(&linker.domain, &desc.permalink, None).await
                else {
                    break;
                };
                let Some((edited, references)) = self
                    .relinked_text(&live.text, linker, spec, relocation, actor, hidden, notes)
                    .await
                else {
                    // The room holds no such reference. When it saves, that
                    // is what lands; when it does not, the source still may.
                    if live.saving {
                        return Ok(None);
                    }
                    break;
                };
                // Whether the room saves is decided again inside the compose,
                // under the lock the compose holds; that is the decision that
                // counts.
                match rooms
                    .compose_while_saving(
                        &linker.domain,
                        &desc.permalink,
                        None,
                        Some(&live.text),
                        edited,
                        actor,
                        None,
                    )
                    .await
                {
                    Ok(_) => {
                        return Ok(Some(Relinked {
                            references,
                            permalink: desc.permalink,
                            live: true,
                        }));
                    }
                    // Somebody is typing. Wait a moment before the next try,
                    // a little longer each time, so a person typing steadily
                    // still leaves a gap the rewrite can compose into.
                    Err(ComposeRefused::Moved) => {
                        if attempt + 1 < super::edit::UNGUARDED_EDIT_ATTEMPTS {
                            tokio::time::sleep(super::edit::LIVE_RETRY_PAUSE * (1 << attempt))
                                .await;
                        }
                        continue;
                    }
                    Err(ComposeRefused::NotSaving) => break,
                }
            }
            tracing::debug!(
                domain = linker.domain.as_str(),
                path = linker.path.as_str(),
                "the room over this linker will not take the move's rewrite, \
                 so it goes to the source and the room merges it on its next save"
            );
        }
        let source = self.read_source(&linker.domain);
        let (replaced, count, _lock) = match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &linker.path);
                let lock = self.lock_file(&abs).await;
                let text = std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })?;
                let Some((replaced, count)) = self
                    .relinked_text(&text, linker, spec, relocation, actor, hidden, notes)
                    .await
                else {
                    return Ok(None);
                };
                write_file(&abs, &replaced)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, linker.domain_id, root, &linker.path)
                    .await?;
                (replaced, count, Some(lock))
            }
            ContentSource::Virtual => {
                let mut attempt = 0usize;
                loop {
                    attempt += 1;
                    let stored = {
                        let store = self.store.lock().await;
                        match store.engram_content(linker.domain_id, &linker.path).await? {
                            Some(text) => text,
                            None => return Ok(None),
                        }
                    };
                    // Compared against the bytes as stored, before the LF
                    // pass below, as an edit compares them.
                    let expected = sha256_hex(stored.as_bytes());
                    let Some((replaced, count)) = self
                        .relinked_text(&stored, linker, spec, relocation, actor, hidden, notes)
                        .await
                    else {
                        return Ok(None);
                    };
                    // The seam an edit's virtual arm has: a token nothing can
                    // match, never retried. See `Engine::fail_next_source_edit`.
                    let armed = self.take_armed_failure();
                    // And the race seam, which is retried like a real race.
                    // See `Engine::lose_next_relink_swap`.
                    let lost = self.take_lost_relink_swap();
                    let expected = if armed || lost {
                        "0".repeat(64)
                    } else {
                        expected
                    };
                    let store = self.store.lock().await;
                    let indexed = self
                        .index_markdown(
                            &*store,
                            linker.domain_id,
                            &linker.path,
                            &replaced,
                            virtual_stamp(&replaced),
                            Some(&expected),
                            true,
                        )
                        .await;
                    drop(store);
                    match indexed {
                        Ok(_) => break (replaced, count, None),
                        Err(e)
                            if !armed
                                && attempt < super::edit::UNGUARDED_EDIT_ATTEMPTS
                                && super::edit::is_stale_edit(&e) =>
                        {
                            tracing::debug!(
                                domain = linker.domain.as_str(),
                                path = linker.path.as_str(),
                                attempt,
                                "a move's rewrite lost a race with another writer; \
                                 applying it again to the stored text"
                            );
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        };
        drop(_lock);
        let permalink = parse_engram(&replaced)
            .ok()
            .and_then(|engram| engram.frontmatter.permalink)
            .filter(|permalink| !permalink.is_empty())
            .unwrap_or_else(|| crystalline_core::path_permalink(&linker.path));
        // The linker's own text changed, possibly in another domain, so its
        // page follows too.
        self.announce(Change::Engram(EngramChanged {
            domain: linker.domain.clone(),
            permalink: permalink.clone(),
            path: linker.path.clone(),
            kind: ChangeKind::Modified,
            from: None,
            checksum: Some(sha256_hex(replaced.as_bytes())),
            actor: Some(actor.to_string()),
            draft_of: None,
            audience: None,
        }));
        Ok(Some(Relinked {
            references: count,
            permalink,
            live: false,
        }))
    }

    /// The linker's text with the moved engram's references rewritten,
    /// stamped and normalized, and how many references changed; `None` when
    /// none did. A relative link, and a bare wikilink spelled as the file's
    /// path, follow the file (`relocation`), and only in the domain it moved
    /// inside, which is the only one either can reach it from.
    #[allow(clippy::too_many_arguments)]
    async fn relinked_text(
        &self,
        text: &str,
        linker: &MoveLinker,
        spec: &Relink<'_>,
        relocation: Option<&Relocation<'_>>,
        actor: &str,
        hidden: &HashSet<String>,
        notes: &mut Option<String>,
    ) -> Option<(String, usize)> {
        // Rewritten as LF, the whole file, like every write.
        let text = crystalline_core::to_lf(text);
        // A lookup that fails shadows nothing, as before this rule existed.
        let shadowed = self
            .shadowed_titles(&text, &linker.domain, spec)
            .await
            .unwrap_or_default();
        let spec = &Relink {
            shadowed: &shadowed,
            ..*spec
        };
        let (mut relinked, mut count) =
            crystalline_core::relink::relink(&text, &linker.domain, &linker.domain, spec);
        let moved_here = relocation.filter(|moved| moved.domain_id == linker.domain_id);
        if let Some(moved) = moved_here
            && let Some(address) = moved.across
        {
            // The engram left for another domain: a relative path cannot
            // follow it there, so the links to it name its address.
            use crystalline_core::relink::NoteLinkTo;
            let whole = crystalline_core::relink::readdress_relative_links(&relinked, &|target| {
                (crystalline_core::reflection::links::resolve_in_domain(&linker.path, target)
                    .as_deref()
                    == Some(moved.from))
                .then(|| NoteLinkTo::Wikilink(address.to_string()))
            });
            let (readdressed, path_count) =
                crystalline_core::relink::readdress_path_wikilinks(&whole.text, &|target| {
                    (target == moved.from).then(|| address.to_string())
                });
            *notes = cross_link_reason(&[], &whole.unwritable, &whole.dropped);
            relinked = readdressed;
            count += whole.changed + path_count;
        } else if let Some(moved) = moved_here {
            // The moved engram itself, when its rebase was deferred to here:
            // its links are rebased rather than respelled.
            let (respelled, respelled_count) = if linker.path == moved.to {
                crystalline_core::relink::rebase_relative_links(&relinked, moved.from, moved.to)
            } else {
                crystalline_core::relink::respell_relative_links(
                    &relinked,
                    &linker.path,
                    moved.from,
                    moved.to,
                )
            };
            let (respelled, path_count) =
                crystalline_core::relink::respell_path_wikilinks(&respelled, moved.from, moved.to);
            relinked = respelled;
            count += respelled_count + path_count;
        }
        // A wikilink spelled as the file's path with a prefix naming its old
        // domain (`[[notes:guides/a.md]]`) binds by path from any domain, so
        // it follows the file to its new path, in its new domain.
        if let Some(moved) = relocation {
            let (readdressed, prefixed_count) =
                crystalline_core::relink::readdress_prefixed_path_wikilinks(
                    &relinked,
                    &|domain, target| {
                        (moved.spellings.contains(domain) && target == moved.from).then(|| {
                            match moved.across {
                                // Written in the domain it moved into, the
                                // path needs no prefix to reach it.
                                Some(_) if linker.domain == moved.to_domain => moved.to.to_string(),
                                Some(_) => format!("{}:{}", moved.to_domain, moved.to),
                                None => format!("{domain}:{}", moved.to),
                            }
                        })
                    },
                );
            relinked = readdressed;
            count += prefixed_count;
        }
        if count == 0 {
            return None;
        }
        let replaced = touch_generated(&relinked, actor, None, now_offset());
        // The rewritten reference passes through the same funnel every other
        // write's final text does: `spec.to_domain` is the destination's
        // local name (moves are localized before they reach here), so
        // without this a linker would carry `[[<local-name>:...]]` even where
        // that local name is only an alias or a non-canonical spelling of the
        // domain's declared name. Called before the store lock is taken,
        // since it may itself need the store.
        let (replaced, _) = self.normalize_domain_spellings(&replaced, hidden).await;
        Some((replaced, count))
    }

    /// Rename a tag to `new`, or (with `merge`) fold it into an existing `new`,
    /// across every engram that carries it, optionally scoped to one domain.
    /// Each affected file is rewritten string-surgically by
    /// [`crystalline_core::retag`]: only the tag tokens change, every other byte
    /// (including the `generated` provenance block) is preserved, so a hygiene
    /// rename never
    /// reflows a file or looks like a fresh edit. Files are the source of truth,
    /// so each rewrite writes the file (or the virtual row) then reindexes it,
    /// which is where the index picks up the new tag identity.
    ///
    /// The two verbs differ only in a precheck: a `rename` refuses when `new`
    /// already exists (that would silently merge, so it points at `tags merge`),
    /// and a `merge` refuses when `new` does not exist yet. `dry_run` reports the
    /// affected engrams without writing anything.
    ///
    /// A non-dry-run `merge` with `record_alias` also records the fold as a tag
    /// alias: it appends `- old -> new` to each affected domain's MANIFEST
    /// `## Tag Aliases` section (creating the section when absent), so a later
    /// search for the old name still finds its engrams through the alias. The
    /// recording is idempotent and permissive: when the pair is already present
    /// the append no-ops and the domain still counts as recorded (a re-merge of a
    /// tag that reappeared must work, never be refused), and a domain with no
    /// MANIFEST lands in `alias_skipped` rather than erroring. When `old` is
    /// already aliased to a different canonical, first-wins parsing keeps the
    /// existing mapping, so a fresh bullet would be inert: the MANIFEST is left
    /// untouched and the domain is surfaced in `alias_conflict` rather than a
    /// false `alias_recorded`. The merge's tag rewrites still proceed either way;
    /// only the recording is skipped. A `rename` records nothing.
    ///
    /// A read-only domain is left out (named in `read_only_skipped`), and a
    /// retag scoped to one is refused.
    pub async fn retag(
        &self,
        old: &str,
        new: &str,
        domain: Option<&str>,
        merge: bool,
        dry_run: bool,
        record_alias: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // A retag of one read-only domain is refused; across every domain it
        // skips the read-only ones and names them (decision D7).
        // A dry run only previews: it meets the read-only check, never the
        // host gate, which may claim a lock.
        if let Some(d) = domain {
            if dry_run {
                self.refuse_read_only_flag(d, &crate::scope::Scope::Unrestricted)
                    .await?;
            } else {
                self.refuse_read_only_domain(d, &crate::scope::Scope::Unrestricted)
                    .await?;
            }
        }
        let old_f = old.trim().to_lowercase();
        let new_f = new.trim().to_lowercase();
        // Only the target name must be a canonical lowercase-with-hyphens tag:
        // the whole point of a rename or merge is to move a non-canonical `old`
        // (an underscore or separator variant that the cluster detection flags)
        // onto a clean name, so `old` only has to be a non-empty folded tag.
        if old_f.is_empty() {
            return Err(EngineError::Invalid(
                "the tag to rename or merge is empty".into(),
            ));
        }
        if !is_lower_hyphen(&new_f) {
            return Err(EngineError::Invalid(format!(
                "target tag '{new_f}' is not a lowercase-with-hyphens tag"
            )));
        }
        if old_f == new_f {
            return Err(EngineError::Invalid(
                "the old and new tag are the same".into(),
            ));
        }

        // What this instance has no registration for is not part of this verb's
        // world: not a tag that exists, not an engram to list, not a file to
        // rewrite. Resolved once, before the first store lock (the lock is not
        // reentrant), and threaded through both reads below.
        //
        // The privacy half of `hidden_for` is deliberately absent: renaming a
        // tag is `Scope::Unrestricted` only, so there is no caller here with a
        // narrower view than the machine's own.
        let unregistered = self.unregistered_domains().await?;

        // Precheck against the vocabulary in scope: a rename must not collide
        // with an existing tag, a merge must land on one.
        let new_exists = {
            let vocab = self.scoped_vocabulary(domain, &unregistered).await?;
            vocab.tags.iter().any(|t| t.name == new_f)
        };
        let scope = match domain {
            Some(d) => format!(" in domain '{d}'"),
            None => String::new(),
        };
        if merge {
            if !new_exists {
                return Err(EngineError::NotFound(format!(
                    "cannot merge into '{new_f}': no engram carries it{scope}"
                )));
            }
        } else if new_exists {
            return Err(EngineError::Conflict(format!(
                "tag '{new_f}' already exists{scope}; use `crystalline tags merge` to combine them"
            )));
        }

        // The engrams carrying the old tag, ordered by domain then path, with
        // every unregistered domain's dropped here at the single point where
        // the list is built: `listed`, the dry run, the rewrite loop, the count
        // and the alias recording all read from it, so one filter covers all of
        // them and no later addition can forget it.
        let (targets, read_only_skipped, hosted_elsewhere_skipped) = {
            let store = self.store.lock().await;
            let mut found = store.engrams_with_tag(&old_f, domain).await?;
            drop(store);
            found.retain(|d| !unregistered.contains(&d.domain));
            // Across every domain, a file domain another instance writes on
            // this shared database is left out and named, never claimed: its
            // files are that instance's. A named domain met the host gate
            // above (or, for a dry run, is previewed as it stands).
            let mut elsewhere: Vec<String> = Vec::new();
            if domain.is_none() {
                // An unattended request sweeps only the domains its run works
                // on (`crate::read_limit`); a named domain meets the same rule
                // in `refuse_read_only_domain` above.
                if let Some(limit) = crate::read_limit::current()
                    && let Some(writes) = &limit.writes
                {
                    if writes.is_empty() {
                        return Err(EngineError::Refused(
                            "this run works on no domain and cannot write to any".into(),
                        ));
                    }
                    found.retain(|d| writes.contains(&d.domain));
                }
                let mut here: HashMap<String, bool> = HashMap::new();
                let mut kept = Vec::with_capacity(found.len());
                for d in found {
                    let writes = match here.get(&d.domain) {
                        Some(known) => *known,
                        None => {
                            let known = self.writes_domain_here(&d.domain, !dry_run).await;
                            here.insert(d.domain.clone(), known);
                            known
                        }
                    };
                    if writes {
                        kept.push(d);
                    }
                }
                found = kept;
                elsewhere = here
                    .into_iter()
                    .filter_map(|(name, writes)| (!writes).then_some(name))
                    .collect();
                elsewhere.sort();
            }
            // A read-only domain is left out here too, for the same reason:
            // one filter covers the dry run, the rewrite and the alias
            // recording into its MANIFEST. Asked once per domain, after the
            // store guard is gone.
            let mut locked: HashMap<String, bool> = HashMap::new();
            let mut kept = Vec::with_capacity(found.len());
            for d in found {
                let is_locked = match locked.get(&d.domain) {
                    Some(known) => *known,
                    None => {
                        let known = self.writes_refused(&d.domain).await;
                        locked.insert(d.domain.clone(), known);
                        known
                    }
                };
                if !is_locked {
                    kept.push(d);
                }
            }
            let mut skipped: Vec<String> = locked
                .into_iter()
                .filter_map(|(name, is_locked)| is_locked.then_some(name))
                .collect();
            skipped.sort();
            (kept, skipped, elsewhere)
        };
        let listed: Vec<Value> = targets
            .iter()
            .map(|d| json!({ "domain": d.domain, "permalink": d.permalink, "path": d.path }))
            .collect();

        if dry_run {
            let mut dry = json!({
                "old": old_f, "new": new_f, "merge": merge, "dry_run": true,
                "engrams": listed, "rewritten": targets.len(),
            });
            if !read_only_skipped.is_empty() {
                dry["read_only_skipped"] = json!(read_only_skipped);
            }
            if !hosted_elsewhere_skipped.is_empty() {
                dry["hosted_elsewhere_skipped"] = json!(hosted_elsewhere_skipped);
            }
            return Ok(dry);
        }

        // Rewrite each engram, mirroring move_engram's file-vs-virtual branches.
        let mut rewritten = 0usize;
        // What each domain saw rewritten, announced once per domain after every
        // store lock is released, through the collapse rule a sync goes
        // through: a tag across a hundred engrams is one `domain` event.
        let mut pending = PendingAnnouncements::new(self);
        let retagged = |desc: &EngramDescriptor, edited: &str| PathChange {
            kind: crystalline_index::PathChangeKind::Modified,
            path: desc.path.clone(),
            from: None,
            from_permalink: None,
            permalink: Some(desc.permalink.clone()),
            checksum: Some(sha256_hex(edited.as_bytes())),
        };
        for desc in &targets {
            match self.read_source(&desc.domain) {
                ContentSource::File { root } => {
                    let abs = join_rel(&root, &desc.path);
                    let Ok(text) = std::fs::read_to_string(&abs) else {
                        continue;
                    };
                    let text = crystalline_core::to_lf(&text);
                    let Some((edited, _)) = crystalline_core::retag(&text, &old_f, &new_f) else {
                        continue;
                    };
                    write_file(&abs, &edited)?;
                    let store = self.store.lock().await;
                    self.reindex_file(&*store, desc.domain_id, &root, &desc.path)
                        .await?;
                    rewritten += 1;
                    pending.push_path(&desc.domain, retagged(desc, &edited));
                }
                ContentSource::Virtual => {
                    let current = {
                        let store = self.store.lock().await;
                        store.engram_content(desc.domain_id, &desc.path).await?
                    };
                    let Some(text) = current else { continue };
                    let text = crystalline_core::to_lf(&text);
                    let Some((edited, _)) = crystalline_core::retag(&text, &old_f, &new_f) else {
                        continue;
                    };
                    let stamp = virtual_stamp(&edited);
                    let store = self.store.lock().await;
                    self.index_markdown(
                        &*store,
                        desc.domain_id,
                        &desc.path,
                        &edited,
                        stamp,
                        None,
                        true,
                    )
                    .await?;
                    rewritten += 1;
                    pending.push_path(&desc.domain, retagged(desc, &edited));
                }
            }
        }

        let mut response = json!({
            "old": old_f, "new": new_f, "merge": merge, "dry_run": false,
            "engrams": listed, "rewritten": rewritten,
        });
        if !read_only_skipped.is_empty() {
            response["read_only_skipped"] = json!(read_only_skipped);
        }
        if !hosted_elsewhere_skipped.is_empty() {
            response["hosted_elsewhere_skipped"] = json!(hosted_elsewhere_skipped);
        }

        // Record the fold as a tag alias in each affected domain's MANIFEST, once
        // per distinct domain in first-seen order. Only a merge records, and only
        // when the caller did not opt out.
        if merge && record_alias {
            let mut domains: Vec<(String, DomainId)> = Vec::new();
            for desc in &targets {
                if !domains.iter().any(|(name, _)| *name == desc.domain) {
                    domains.push((desc.domain.clone(), desc.domain_id));
                }
            }

            let mut alias_recorded: Vec<String> = Vec::new();
            let mut alias_skipped: Vec<String> = Vec::new();
            let mut alias_conflict: Vec<String> = Vec::new();
            let mut virtual_manifest_changed = false;
            for (name, domain_id) in &domains {
                match self.read_source(name) {
                    ContentSource::File { root } => {
                        let abs = join_rel(&root, "MANIFEST.md");
                        let Ok(text) = std::fs::read_to_string(&abs) else {
                            alias_skipped.push(name.clone());
                            continue;
                        };
                        let text = crystalline_core::to_lf(&text);
                        match decide_alias_record(&text, &old_f, &new_f) {
                            AliasRecord::Recorded(edited) => {
                                write_file(&abs, &edited)?;
                                let store = self.store.lock().await;
                                self.reindex_file(&*store, *domain_id, &root, "MANIFEST.md")
                                    .await?;
                                alias_recorded.push(name.clone());
                                pending.push_path(name, manifest_change(&edited));
                            }
                            AliasRecord::AlreadyPresent => alias_recorded.push(name.clone()),
                            AliasRecord::Conflict => alias_conflict.push(name.clone()),
                        }
                    }
                    ContentSource::Virtual => {
                        let current = {
                            let store = self.store.lock().await;
                            store.engram_content(*domain_id, "MANIFEST.md").await?
                        };
                        let Some(text) = current else {
                            alias_skipped.push(name.clone());
                            continue;
                        };
                        let text = crystalline_core::to_lf(&text);
                        match decide_alias_record(&text, &old_f, &new_f) {
                            AliasRecord::Recorded(edited) => {
                                let stamp = virtual_stamp(&edited);
                                let store = self.store.lock().await;
                                self.index_markdown(
                                    &*store,
                                    *domain_id,
                                    "MANIFEST.md",
                                    &edited,
                                    stamp,
                                    None,
                                    true,
                                )
                                .await?;
                                virtual_manifest_changed = true;
                                alias_recorded.push(name.clone());
                                pending.push_path(name, manifest_change(&edited));
                            }
                            AliasRecord::AlreadyPresent => alias_recorded.push(name.clone()),
                            AliasRecord::Conflict => alias_conflict.push(name.clone()),
                        }
                    }
                }
            }

            // A rewritten virtual MANIFEST may have changed its routing bullets,
            // so refresh the cache once, after every store lock is released.
            if virtual_manifest_changed {
                self.refresh_routing_cache().await;
            }

            if let Value::Object(map) = &mut response {
                map.insert("alias_recorded".to_string(), json!(alias_recorded));
                map.insert("alias_skipped".to_string(), json!(alias_skipped));
                map.insert("alias_conflict".to_string(), json!(alias_conflict));
            }
        }

        // Retag carries no scope and stamps nobody. Announced here, or on any
        // early exit above, for every row that landed.
        drop(pending);
        Ok(response)
    }

    /// Refuse a move whose destination path is already taken, checking disk for
    /// a file domain and the database for a virtual one.
    async fn ensure_dest_free(
        &self,
        dest_source: &ContentSource,
        dest_domain: &str,
        dest_rel: &str,
        src_source: &ContentSource,
        src_rel: &str,
        cross: bool,
    ) -> Result<()> {
        let taken = match dest_source {
            // The source file itself at the destination (a letter-case rename
            // on a case-insensitive disk) is not a collision.
            ContentSource::File { root } => {
                let dest_abs = join_rel(root, dest_rel);
                // Never across domains: overlapping roots are not one domain.
                let itself = !cross
                    && matches!(src_source, ContentSource::File { root: r }
                        if is_case_only_rename(&join_rel(r, src_rel), &dest_abs, src_rel, dest_rel));
                let taken = dest_abs.exists() && !itself;
                if !taken && !itself {
                    // A destination whose file is missing here may still
                    // be the row of an engram only the previous host of a
                    // host move has.
                    let dest_id = self.store.lock().await.domain_id(dest_domain).await?;
                    if let Some(dest_id) = dest_id {
                        self.refuse_only_elsewhere(
                            dest_domain,
                            dest_id,
                            ElsewhereRow::Engram(dest_rel),
                        )
                        .await?;
                    }
                }
                taken
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let dest_id = store
                    .upsert_domain(dest_domain, None, DomainKind::Virtual)
                    .await?;
                store.engram_content(dest_id, dest_rel).await?.is_some()
            }
        };
        if taken {
            return Err(EngineError::Conflict(format!(
                "destination '{dest_rel}' already exists in domain '{dest_domain}'"
            )));
        }
        Ok(())
    }
}

/// A MANIFEST rewrite as a path change: its permalink is left to the
/// announce's path fallback, since the alias recording never touches it.
fn manifest_change(edited: &str) -> PathChange {
    PathChange {
        kind: crystalline_index::PathChangeKind::Modified,
        path: "MANIFEST.md".to_string(),
        from: None,
        from_permalink: None,
        permalink: parse_engram(edited)
            .ok()
            .and_then(|engram| engram.frontmatter.permalink)
            .filter(|permalink| !permalink.is_empty()),
        checksum: Some(sha256_hex(edited.as_bytes())),
    }
}

impl Engine {
    /// The moved engram's whole text, frontmatter included, read from its
    /// source of truth. Stricter than the read path on purpose: a move
    /// re-emits the file at the destination, and a file domain's stored
    /// `content` column holds the body only, so falling back to it would strip
    /// the frontmatter off the engram that lands. Failing loudly is the only
    /// honest option; the store is the source of truth for a virtual domain,
    /// so only that kind reads from it.
    async fn move_source_text(
        &self,
        src_source: &ContentSource,
        src: &EngramDescriptor,
        domain: &str,
    ) -> Result<String> {
        match src_source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &src.path);
                std::fs::read_to_string(&abs).map_err(|e| {
                    EngineError::NotFound(format!(
                        "the source file for '{}' at {} is unreadable ({e}); resync '{domain}' and retry the move",
                        src.permalink,
                        abs.display(),
                    ))
                })
            }
            ContentSource::Virtual => self.load_content(src_source, src).await,
        }
    }
}

/// What [`Engine::move_draft_locked`] moves, and for whom.
struct DraftMove<'a> {
    who: &'a str,
    src: &'a EngramDescriptor,
    src_source: &'a ContentSource,
    dest_rel: &'a str,
    cross: bool,
    asked: Option<&'a str>,
    mover: &'a str,
}

/// A move's room guard once the move has committed: from then on the
/// document is at its new address whatever a later step answers, so a way
/// out that drops this tells the room `moved` (in a task, since a drop
/// cannot wait) instead of the `internal` an uncommitted move's guard sends.
struct TellMoved(Option<MoveClose>);

impl Drop for TellMoved {
    fn drop(&mut self) {
        if let Some(closing) = self.0.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(closing.moved());
        }
    }
}
