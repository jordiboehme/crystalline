//! Renaming a domain on this machine: one engine operation that pauses the
//! domain, moves every store keyed by its name step by step under a journal,
//! and writes the configuration last, so a crash after any step is finished
//! by the next engine before it serves.

use crystalline_core::manifest::domain_name_at;

use super::*;
use crate::rename::{
    LeftBehind, RENAME_WAIT, RelinkCount, RelinkReport, RenameCaller, RenameJournal, RenameOwner,
    RenameStep, WriteTicket, move_state_dir,
};

/// Frees the one-rename slot when the rename that took it ends, however it
/// ends.
struct RenameSlot<'a> {
    slot: &'a std::sync::Mutex<Option<(String, String, bool)>>,
}

impl Drop for RenameSlot<'_> {
    fn drop(&mut self) {
        *self.slot.lock().unwrap() = None;
    }
}

/// What the relink step of a full rename did while it ran; the journal keeps
/// it as a [`RelinkReport`] once the step is done.
#[derive(Default)]
struct RenameOutcome {
    /// Per domain, in name order: the engrams rewritten and the references
    /// respelled in them.
    rewritten: BTreeMap<String, (usize, usize)>,
    /// The links the relink step found and did not respell.
    left_behind: Vec<LeftBehind>,
}

impl RenameOutcome {
    fn into_report(self) -> RelinkReport {
        RelinkReport {
            rewritten: self
                .rewritten
                .into_iter()
                .map(|(domain, (engrams, references))| RelinkCount {
                    domain,
                    engrams,
                    references,
                })
                .collect(),
            left_behind: self.left_behind,
        }
    }
}

impl Engine {
    /// Rename a domain everywhere: the MANIFEST's `domain_name`, this
    /// machine's name (recorded as chosen), and every link that spells any
    /// former name (the local name, the canonical name, every alias) in the
    /// domains this caller can write. A team domain takes the rewrites as
    /// local changes for its next share, a reviewing domain as the caller's
    /// drafts, the MANIFEST included; no proposal is opened. Links in domains
    /// the caller can only read are left as they are and listed under
    /// `left_behind`; every former name stays an alias here, so they keep
    /// resolving on this machine.
    ///
    /// `local_only` is [`Engine::rename_domain_local`] with the name recorded
    /// as chosen: MANIFEST and content untouched.
    ///
    /// Refused before any step for everything the local rename refuses, and
    /// when the MANIFEST cannot be written here, which names `--local`.
    ///
    /// The report: `{ domain, previous, local_only, manifest_written,
    /// manifest_draft, rewritten: [{domain, engrams, references}],
    /// left_behind: [{domain, path, references}], aliases, shadows, moved }`,
    /// plus a `note` when something is shadowed or when the MANIFEST's
    /// frontmatter cannot take the new name key by key (flow style), which
    /// leaves the MANIFEST as it is and reports `manifest_written: false`. A
    /// local rename answers the same shape, with nothing written and nothing
    /// rewritten.
    pub async fn rename_domain(
        &self,
        domain: &str,
        new: &str,
        local_only: bool,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.rename_domain_as(domain, new, NameOrigin::Explicit, local_only, scope)
            .await
    }

    /// Rename a domain on this machine only: its local name, and every store
    /// keyed by it (the index row, the visibility records, the `origins/` and
    /// `overlays/` state folders, the provision receipt and the
    /// configuration). The MANIFEST and the content are untouched; the old
    /// local name, the canonical name and every alias become aliases of the
    /// new name, so every link spelled with them keeps resolving.
    ///
    /// The domain is paused while it moves: a write into it waits (up to
    /// 30 s) and then lands under the new name, a sync or a watcher pass
    /// skips it, and the sync at the end covers what they skipped. A journal
    /// is written before the first step and after every one, so a crash is
    /// finished by [`Engine::recover_rename_journal`] before the next serve.
    ///
    /// Refused, before any step: on a read-only instance; for a caller who
    /// does not own the domain; for a domain an environment variable defines;
    /// for an invalid name or one another domain registers here; while
    /// another rename is running; while another live instance serves the same
    /// index; and when state a removed domain left under the new name is in
    /// the way. Renaming onto a name another domain declares in its MANIFEST
    /// or keeps as an alias is allowed; the report names that domain under
    /// `shadows`.
    ///
    /// The report is [`Engine::rename_domain`]'s.
    pub async fn rename_domain_local(
        &self,
        old: &str,
        new: &str,
        origin: NameOrigin,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.rename_domain_as(old, new, origin, true, scope).await
    }

    /// The one rename both entry points reach.
    async fn rename_domain_as(
        &self,
        old: &str,
        new: &str,
        origin: NameOrigin,
        local_only: bool,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // A rename moves this machine's state folders and writes its
        // configuration under a journal in the state directory: only the
        // process that holds that directory runs one, so no two processes
        // move the same folders or run one journal at once.
        if !self.holds_state_dir() {
            return Err(EngineError::Conflict(
                "another Crystalline process (the daemon) holds this machine's state directory, \
                 which a rename needs; rename through it, or stop it with `crystalline ctl \
                 shutdown` and rename again"
                    .to_string(),
            ));
        }
        // Nor while this machine's own index cannot be named: nothing could
        // tell whether the engine opened it.
        if let Some(crate::rename::MachineOwner::Unknown(reason)) = &self.machine_owner {
            return Err(EngineError::Conflict(format!(
                "this machine's own index and configuration cannot be named ({}), so renames \
                 are off in this Crystalline. Nothing was renamed. Fix the configuration so it \
                 loads, then restart the daemon or the MCP server",
                if may_see_server_paths(scope) {
                    reason.as_str()
                } else {
                    "its configuration does not load"
                }
            )));
        }
        // Not over an index `--db` or `--config` named instead of this
        // machine's own: the steps would move this machine's state folders
        // and write its configuration while its own index keeps the old
        // name.
        if let Some(why) = self.off_machine().await? {
            return Err(EngineError::Conflict(format!(
                "a rename moves this machine's state folders and writes its configuration, so \
                 it runs only against this machine's own index and configuration, and {}. \
                 Nothing was renamed. Rename in a Crystalline started without --db and --config",
                if may_see_server_paths(scope) {
                    why
                } else {
                    "this instance opened another index or configuration".to_string()
                }
            )));
        }
        let hidden = self.hidden_for(scope).await?;
        // Privacy alone, never `hidden_for`'s extra "an index row this
        // instance has no registration for" names: the still-running and
        // unfinished-journal refusals below name a rename OTHER THAN this
        // one, and its domain can be a genuinely registered one that reads
        // as orphaned for exactly the moment a paused rename holds it -
        // the index row already renamed, the configuration not yet caught
        // up. `hidden_for`'s extra set would call that domain hidden from
        // every caller, admin included, and swap in the generic wording
        // for a transient sync state rather than for an actual privacy
        // rule. Privacy is the only thing worth hiding one of a rename's
        // own domains over.
        let privacy_hidden = self.hidden_domains(scope).await?.unwrap_or_default();
        let table = self.name_table_now().await;
        let old = names::localize_in(&table, old, &hidden);
        let old = old.as_str();
        self.require_domain_owner_refusing(old, scope, Engine::rename_refusal(old))
            .await?;
        if let Some(env) = self.overlay.env_domain(old) {
            return Err(EngineError::Conflict(format!(
                "domain '{old}' is defined by the environment variable {}; rename the variable \
                 to rename the domain",
                env.var
            )));
        }
        validate_domain_name(new).map_err(EngineError::Invalid)?;
        if new == old {
            return Err(EngineError::Invalid(format!(
                "'{new}' is already this domain's name here; pick a different name"
            )));
        }

        let _slot = self.take_rename_slot(old, new, local_only, &privacy_hidden)?;
        let _admin = self.domain_admin().await;
        let _fence = self.fence_joins().await;
        let state_dir = self.journal_state_dir()?;
        if let Some(journal) = RenameJournal::load(&state_dir).map_err(io_error)? {
            // The same rename a crash stopped: finish it rather than refuse.
            // The pause starts where `finish_rename` needs it.
            if journal.old == old && journal.new == new && journal.local_only == local_only {
                // Finished only against the index it was started on: a
                // command that opened another one (`--db`, `--config`)
                // would move this machine's state and configuration while
                // the index the rename belongs to keeps the old name.
                if let Some(why) = self.foreign_journal(&journal, &state_dir).await? {
                    return Err(EngineError::Conflict(format!(
                        "the rename of '{old}' to '{new}' that has not finished belongs to \
                         another index{}; {}",
                        if may_see_server_paths(scope) {
                            format!(" ({why})")
                        } else {
                            String::new()
                        },
                        finish_elsewhere_hint(&journal)
                    )));
                }
                let _origins = self.lock_both_origins(old, new).await;
                // A resumed journal may already have carried the old name's
                // privacy records over, so who could read it is no longer
                // known: its events fail closed, to the machine owner only.
                // Taken before `close_editors`, whose room saves land under
                // the old name.
                let _captured =
                    self.capture_audience(old, DomainAudience::Accounts(HashSet::new()));
                self.close_editors(old).await;
                let mut report = self
                    .finish_rename(
                        journal,
                        &state_dir,
                        true,
                        DomainAudience::Accounts(HashSet::new()),
                    )
                    .await?;
                if let Value::Object(map) = &mut report {
                    map.entry("shadows").or_insert_with(|| json!([]));
                }
                return Ok(report);
            }
            // A journal a crash left behind for a DIFFERENT rename: naming
            // its domains would tell this caller about a rename of a domain
            // it may not even see, the same class of leak as the taken-name
            // refusal above.
            return Err(EngineError::Conflict(
                if privacy_hidden.contains(&journal.old) || privacy_hidden.contains(&journal.new) {
                    "a rename is still finishing; try again in a moment".to_string()
                } else {
                    format!(
                        "the rename of '{}' to '{}' has not finished; {}, then rename again",
                        journal.old,
                        journal.new,
                        finish_hint(&journal)
                    )
                },
            ));
        }

        let entry = self.domain_entry(old)?;
        if self.registered_domain_entries_now().await.contains_key(new) {
            // A name a private domain hidden from this caller already holds
            // answers no differently from one nobody holds: naming it "already
            // a domain here" would confirm a domain this caller cannot see
            // exists under exactly that name, which is the existence oracle
            // `require_domain` refuses everywhere else on this surface. The
            // status is the same 409 either way; only the wording that would
            // disclose something narrows.
            return Err(EngineError::Conflict(if hidden.contains(new) {
                format!("'{new}' cannot be used as a name here")
            } else {
                format!(
                    "'{new}' is already a domain here; pick another name or rename that domain \
                     first"
                )
            }));
        }
        let reviewing_draft = if local_only {
            false
        } else {
            self.refuse_unwritable_manifest(old, new).await?
        };
        // Judged on the text the MANIFEST step would edit: in a reviewing
        // domain the caller's draft when there is one. A kept MANIFEST is not
        // drafted.
        let manifest_kept = !local_only && !self.manifest_takes_name(old, new, scope).await?;
        let manifest_draft = reviewing_draft && !manifest_kept;
        self.refuse_shared_index(old).await?;
        self.refuse_leftovers(new, &state_dir, scope).await?;
        // What the journal belongs to, read before anything is paused.
        let owner = self.rename_owner(&state_dir).await?;
        // Who could read the old name, read while the privacy records still
        // carry it and before anything is paused (ruled 2026-09-27): the
        // rename's event for the old name carries this snapshot, never the
        // registry after the rename re-keyed it.
        let old_audience = self.domain_audience(old).await;
        // Every event under the old name from here on (the MANIFEST and
        // relink edits inside it, the room saves `close_editors` lands) and
        // every one the ring holds for it carries the snapshot: a path under
        // the old name leaks the name as surely as the domain event does.
        let _captured = self.capture_audience(old, old_audience.clone());

        // An origin pull or share of this domain finishes first, and none
        // starts until the rename is done, under either name: the lock is
        // keyed by name, so both are held.
        let _origins = self.lock_both_origins(old, new).await;

        // Records a removed domain left under the new name are stale: a live
        // domain there was refused above. Forgotten before any step, so a
        // stale owner never gains the renamed domain, whether or not it has
        // records of its own to carry over.
        if let Some(access) = self.rename_access().await? {
            access.forget_domain(new).await.map_err(|e| {
                EngineError::Internal(format!(
                    "clearing stale records under '{new}' before the rename: {e:#}"
                ))
            })?;
        }

        let table = self.name_table_now().await;
        // A domain hidden from this caller must name no spelling of itself
        // anywhere in the report: `shadowed_by` reads the instance-wide
        // table, which knows nothing about who may see what, so its answer
        // is filtered by the same `hidden` set the caller's own name was
        // localized against above.
        let shadows: Vec<String> = shadowed_by(&table, old, new)
            .into_iter()
            .filter(|shadowed| !hidden.contains(shadowed))
            .collect();
        let canonical = self.declared_domain_name(old, &entry);
        let mut old_spellings: Vec<String> = vec![old.to_string()];
        if let Some(canonical) = table.canonical(old) {
            old_spellings.push(canonical.to_string());
        }
        old_spellings.extend(canonical.iter().cloned());
        old_spellings.extend(entry.aliases.iter().cloned());
        let old_spellings = distinct_without(old_spellings, new);
        // Only a spelling that reaches this domain today is this domain's to
        // respell: a declared name another local name shadows, a contested
        // one or an alias the table dropped belongs to some other domain or
        // to none.
        let (relink_spellings, writable) = if local_only {
            (Vec::new(), Vec::new())
        } else {
            let reaching: Vec<String> = old_spellings
                .iter()
                .filter(|s| table.resolve(s) == Some(old))
                .cloned()
                .collect();
            (reaching, self.writable_domains(scope).await?)
        };

        // Rooms save their last text while the domain is still registered
        // under its old name, and a join names the domain it was opened in.
        // This has to come before the pause and the drain, not after: a
        // room's last save is a write into the domain, and once the domain is
        // paused that write would wait on the very rename that is waiting for
        // it. So a rename the drain then refuses has already closed the
        // editors, and its refusal says so. A full rename closes them before
        // its MANIFEST and relink steps too, so those edits reach the file
        // rather than a room that is about to close.
        let closed_editors = self.close_editors(old).await;

        // A rename of this machine's name only pauses before its journal, so
        // a busy domain refuses with nothing started. A full rename pauses
        // after its MANIFEST and relink steps, which write into the domain
        // like any other edit and would otherwise wait on its own pause.
        if local_only && !self.pause_and_drain(old, new).await {
            let closed = if closed_editors {
                " Its open editors were closed and reopen on the next edit."
            } else {
                ""
            };
            return Err(EngineError::Conflict(format!(
                "domain '{old}' is busy with a write that has not finished; try again in a \
                 moment.{closed}"
            )));
        }
        let journal = RenameJournal {
            version: 1,
            old: old.to_string(),
            new: new.to_string(),
            local_only,
            origin,
            old_spellings,
            relink_spellings,
            // What the MANIFEST says once the rename is done: the new name,
            // unless the write is a draft the folder does not carry yet.
            canonical: if local_only || manifest_draft || manifest_kept {
                canonical
            } else {
                Some(new.to_string())
            },
            caller: rename_caller(scope),
            writable,
            manifest_draft,
            manifest_kept,
            relinked: None,
            owner: Some(owner),
            done: Vec::new(),
        };
        if let Err(e) = journal.save(&state_dir) {
            self.rename_pause.resume(&[old, new]);
            return Err(io_error(e));
        }
        let mut report = self
            .finish_rename(journal, &state_dir, true, old_audience)
            .await?;
        if let Value::Object(map) = &mut report {
            if !shadows.is_empty() {
                let shadow_note = format!(
                    "'{new}' was the name {} answered to; it now reaches this domain. \
                     Rename {} or change its domain_name to line them up.",
                    quoted_list(&shadows),
                    if shadows.len() == 1 { "it" } else { "them" }
                );
                // After the MANIFEST note finish_rename may have put there.
                let note = match map.get("note").and_then(Value::as_str) {
                    Some(first) => format!("{first} {shadow_note}"),
                    None => shadow_note,
                };
                map.insert("note".to_string(), json!(note));
            }
            map.insert("shadows".to_string(), json!(shadows));
        }
        Ok(report)
    }

    /// Finish a rename a crash left behind: every step the journal does not
    /// list as done, then the configuration, then the journal goes. Called
    /// before serving and before the first sync, by the daemon and by both
    /// standalone openers. No journal: `Ok(None)`. A journal that belongs to
    /// another index, configuration or state directory than this engine's
    /// is left alone with a warning, also `Ok(None)`.
    ///
    /// Runs on a read-only instance too: the rename was started by a
    /// writable one, and a half-renamed domain serves nothing correctly.
    pub async fn recover_rename_journal(&self) -> Result<Option<Value>> {
        // An engine that knows no state directory cannot have written a
        // journal.
        let Ok(state_dir) = self.journal_state_dir() else {
            return Ok(None);
        };
        let Some(journal) = RenameJournal::load(&state_dir).map_err(io_error)? else {
            return Ok(None);
        };
        if !self.holds_state_dir() {
            tracing::warn!(
                "the rename of domain '{}' to '{}' that an earlier run left half done is left \
                 to the process that holds this machine's state directory",
                journal.old,
                journal.new
            );
            return Ok(None);
        }
        // Nothing is finished over an index `--db` or `--config` named
        // instead of this machine's own, and no journal recorded against
        // anything but this machine's own index and configuration: `crystalline
        // doctor` names such a journal and how to finish or drop it.
        if let Some(why) = self.off_machine().await? {
            let hint = if matches!(
                self.machine_owner,
                Some(crate::rename::MachineOwner::Unknown(_))
            ) {
                "fix this machine's configuration so it loads, then restart the daemon or the \
                 MCP server, which finishes it before serving"
                    .to_string()
            } else {
                finish_elsewhere_hint(&journal)
            };
            tracing::warn!(
                "the rename of domain '{}' to '{}' that an earlier run left half done is left \
                 alone here: {why}. To finish it, {hint}",
                journal.old,
                journal.new,
            );
            return Ok(None);
        }
        if let Some(crate::rename::MachineOwner::Known(machine)) = &self.machine_owner
            && !journal
                .owner
                .as_ref()
                .is_some_and(|owner| owner.same_as(machine))
        {
            tracing::warn!(
                "the rename of domain '{}' to '{}' that an earlier run left half done is left \
                 alone: it was recorded against {}, which is not this machine's own {}. \
                 `crystalline doctor` says how to finish or drop it",
                journal.old,
                journal.new,
                journal
                    .owner
                    .as_ref()
                    .map(RenameOwner::describe)
                    .unwrap_or_else(|| "no recorded index".to_string()),
                machine.describe()
            );
            return Ok(None);
        }
        // A journal another index's rename left behind is not this engine's
        // to finish: its steps would move this machine's state folders and
        // configuration while the index it belongs to keeps the old name.
        if let Some(why) = self.foreign_journal(&journal, &state_dir).await? {
            tracing::warn!(
                "the rename of domain '{}' to '{}' that an earlier run left half done is left \
                 alone here: {why}. To finish it, {}",
                journal.old,
                journal.new,
                finish_elsewhere_hint(&journal)
            );
            return Ok(None);
        }
        // No caller to protect here - this runs before the daemon serves
        // anyone - so nothing is hidden and a refusal (there is no live
        // caller to read one) would carry the full detail regardless.
        let _slot = self.take_rename_slot(
            &journal.old,
            &journal.new,
            journal.local_only,
            &HashSet::new(),
        )?;
        let _admin = self.domain_admin().await;
        let _fence = self.fence_joins().await;
        let _origins = self.lock_both_origins(&journal.old, &journal.new).await;
        // No pause here: `finish_rename` pauses the domain before the first
        // step that moves it, after the MANIFEST and relink steps a full
        // rename may still owe.
        tracing::info!(
            "finishing the rename of domain '{}' to '{}' that an earlier run left half done",
            journal.old,
            journal.new
        );
        // No sync here: every opener runs its own first sync after this,
        // and the daemon calls this before its socket binds, where a scan of
        // the whole domain would hold every client back.
        // Finished at a start, after the records may already have moved: who
        // could read the old name is no longer known, so its events fail
        // closed, to the machine owner only.
        let nobody = DomainAudience::Accounts(HashSet::new());
        let _captured = self.capture_audience(&journal.old, nobody.clone());
        self.finish_rename(journal, &state_dir, false, nobody)
            .await
            .map(Some)
    }

    /// [`Engine::recover_rename_journal`] for an opener: the daemon, the
    /// embedded MCP stack and the standalone command engine call it before
    /// anything reads a name, each only while it holds the ownership of the
    /// state directory, so no two processes run one journal. A failure is
    /// logged rather than returned: a rename that cannot be finished keeps
    /// its one domain paused and says why, and every other domain is served.
    /// A rename it finished is logged and its report answered, for the one
    /// caller that was about to send that same rename again.
    pub async fn finish_leftover_rename(&self) -> Option<Value> {
        match self.recover_rename_journal().await {
            Ok(Some(report)) => {
                tracing::info!(
                    "finished renaming domain '{}' to '{}', which an earlier run left half done",
                    report["previous"].as_str().unwrap_or_default(),
                    report["domain"].as_str().unwrap_or_default()
                );
                Some(report)
            }
            Ok(None) => None,
            Err(err) => {
                tracing::error!(
                    "a domain rename an earlier run left half done could not be finished ({err}); \
                 the next start tries again. If it stopped after its MANIFEST and link steps, \
                 that domain stays paused until then: every read of it waits 30 s before it is \
                 answered, and every write to it is refused"
                );
                None
            }
        }
    }

    /// Say whether this process holds the ownership of the state directory
    /// right now. Every engine starts out holding it; the standalone command
    /// engine starts without it, and its opener sets it while it has taken
    /// the lock. A rename journal is run, and MANIFEST names adopted, only
    /// while it is held.
    pub fn set_holds_state_dir(&self, held: bool) {
        self.holds_state_dir
            .store(held, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether this process holds the ownership of the state directory.
    pub(crate) fn holds_state_dir(&self) -> bool {
        self.holds_state_dir
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether a rename journal is waiting in this engine's state directory,
    /// whoever it belongs to. Cheap: a standalone opener asks this before it
    /// takes the ownership lock a recovery needs.
    pub fn has_leftover_rename(&self) -> bool {
        self.journal_state_dir()
            .is_ok_and(|dir| RenameJournal::path(&dir).exists())
    }

    /// The index, configuration and state directory this engine runs
    /// against, as a journal it writes records them.
    async fn rename_owner(&self, state_dir: &Path) -> Result<RenameOwner> {
        let info = {
            let store = self.store.lock().await;
            store.store_info().await?
        };
        Ok(RenameOwner::new(
            info.db_path.as_deref(),
            self.config_path.as_deref(),
            state_dir,
        ))
    }

    /// Why the index, configuration or state directory this engine opened
    /// are not this machine's own, naming each part that differs, or why
    /// this machine's own could not be named at all; `None` when they are
    /// this machine's own, or when the opener named no machine owner
    /// ([`Engine::with_machine_owner`], an engine a test builds).
    pub(crate) async fn off_machine(&self) -> Result<Option<String>> {
        let machine = match &self.machine_owner {
            None => return Ok(None),
            Some(crate::rename::MachineOwner::Unknown(reason)) => {
                return Ok(Some(format!(
                    "this machine's own index and configuration cannot be named ({reason})"
                )));
            }
            Some(crate::rename::MachineOwner::Known(machine)) => machine,
        };
        let state_dir = self.journal_state_dir()?;
        let here = self.rename_owner(&state_dir).await?;
        let differences = here.differences_from(machine);
        Ok((!differences.is_empty()).then(|| differences.join(", and ")))
    }

    /// `None` when `journal` belongs to the index, configuration and state
    /// directory this engine opened; otherwise why it does not.
    async fn foreign_journal(
        &self,
        journal: &RenameJournal,
        state_dir: &Path,
    ) -> Result<Option<String>> {
        let here = self.rename_owner(state_dir).await?;
        Ok(match &journal.owner {
            Some(owner) if owner.same_as(&here) => None,
            Some(owner) => Some(format!(
                "it belongs to {}, and this command opened {}",
                owner.describe(),
                here.describe()
            )),
            None => Some(format!(
                "it does not say which index it belongs to, and this command opened {}",
                here.describe()
            )),
        })
    }

    /// Whether `domain` is being renamed right now, under its old name or its
    /// new one.
    pub fn is_renaming(&self, domain: &str) -> bool {
        self.rename_pause.is_paused(domain)
    }

    /// Hold the next write into any domain right after it is counted, until
    /// the answered hold is released: a write already running when a rename
    /// starts.
    #[cfg(any(test, feature = "testing"))]
    pub fn hold_next_write(&self) -> Arc<crate::rename::RenameHold> {
        let hold = Arc::new(crate::rename::RenameHold::default());
        *self.write_hold.lock().unwrap() = Some(hold.clone());
        hold
    }

    /// Arm a failure right after `step` of the next rename, leaving its
    /// journal behind as a crash would; `None` disarms it.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_rename_after(&self, step: Option<RenameStep>) {
        *self.rename_fail_after.lock().unwrap() = step;
    }

    /// Wait `limit` instead of 30 s for the writes running in a domain a
    /// rename is about to move; `None` restores the real limit.
    #[cfg(any(test, feature = "testing"))]
    pub fn set_rename_drain_wait(&self, limit: Option<std::time::Duration>) {
        *self.rename_drain_wait.lock().unwrap() = limit;
    }

    /// Hold the next rename right after `step` until the answered hold is
    /// released.
    #[cfg(any(test, feature = "testing"))]
    pub fn hold_rename_after(&self, step: RenameStep) -> Arc<crate::rename::RenameHold> {
        let hold = Arc::new(crate::rename::RenameHold::default());
        *self.rename_hold.lock().unwrap() = Some((step, hold.clone()));
        hold
    }

    /// Wait while a rename has paused one of the domains `spellings` name,
    /// through the table as it stands. A no-op when nothing is paused.
    /// `Err` with the conflict a caller answers when the wait runs out.
    pub(super) async fn wait_for_renames(&self, spellings: &[String]) -> Result<()> {
        if !self.rename_pause.any() {
            return Ok(());
        }
        let table = self.names.read().unwrap().1.clone();
        let mut names: Vec<String> = spellings.to_vec();
        names.extend(
            spellings
                .iter()
                .filter_map(|s| table.resolve(s).map(str::to_string)),
        );
        self.rename_pause
            .wait_clear(&names, RENAME_WAIT)
            .await
            .map(|_| ())
            .map_err(|busy| {
                // Answered in the caller's own words: the local name a
                // canonical name or alias resolves to may be one of a domain
                // this caller may not see.
                let typed = spellings
                    .iter()
                    .find(|s| **s == busy || table.resolve(s) == Some(busy.as_str()))
                    .cloned()
                    .unwrap_or(busy);
                renaming_conflict(typed)
            })
    }

    /// Count one write into `name` for as long as the ticket lives, waiting
    /// first while a rename has the domain paused. A write that waited for a
    /// rename of the very name it holds is refused, since the name it holds
    /// is gone; it is sent again under the name it reaches now.
    pub(crate) async fn enter_write(&self, name: &str) -> Result<WriteTicket<'_>> {
        loop {
            let waited = self
                .rename_pause
                .wait_clear(&[name.to_string()], RENAME_WAIT)
                .await
                .map_err(renaming_conflict)?;
            if waited && !self.registered_domain_names().contains(name) {
                return Err(EngineError::Conflict(format!(
                    "domain '{name}' was renamed while this request waited; send it again"
                )));
            }
            if let Some(ticket) = self.rename_pause.try_enter(name) {
                #[cfg(any(test, feature = "testing"))]
                {
                    let hold = self.write_hold.lock().unwrap().take();
                    if let Some(hold) = hold {
                        hold.hold().await;
                    }
                }
                return Ok(ticket);
            }
        }
    }

    /// Count one pass of a sync or the watcher into `name`, or `None` when a
    /// rename has it paused: the pass is skipped, and the sync that ends the
    /// rename covers it.
    pub(super) fn try_enter_sync(&self, name: &str) -> Option<WriteTicket<'_>> {
        self.rename_pause.try_enter(name)
    }

    /// Every name a rename in flight or a leftover journal holds, which a
    /// sweep of unregistered domains must count as registered.
    pub(super) fn names_being_renamed(&self) -> Vec<String> {
        let mut names = self.rename_pause.names();
        if let Ok(dir) = self.journal_state_dir()
            && let Ok(Some(journal)) = RenameJournal::load(&dir)
        {
            names.push(journal.old);
            names.push(journal.new);
        }
        names
    }

    /// Whether a spelling push has to wait for a rename's config step.
    pub(super) fn names_frozen(&self) -> bool {
        self.names_frozen.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Run every step of `journal` it does not list as done, re-key what
    /// memory holds, write the configuration, publish the names, drop the
    /// journal, lift the pause and, when `sync_after`, sync the domain under
    /// its new name. Announces both names: the old one under `old_audience`,
    /// captured before the first step (or nobody, for a journal resumed
    /// after the records may have moved), the new one under the ordinary
    /// per-session check, since its records were re-keyed, not destroyed.
    async fn finish_rename(
        &self,
        mut journal: RenameJournal,
        state_dir: &Path,
        sync_after: bool,
        old_audience: DomainAudience,
    ) -> Result<Value> {
        let (old, new) = (journal.old.clone(), journal.new.clone());
        let mut moved: Vec<&'static str> = Vec::new();
        for &step in journal.steps() {
            let moves = !matches!(step, RenameStep::Manifest | RenameStep::Relink);
            // The domain is paused before the first step that moves it, and
            // not before: the MANIFEST and relink steps of a full rename
            // write into the domain like any edit, and would wait on the
            // pause. A rename that paused earlier (every local one) or a
            // domain a stopped rename left paused goes straight on.
            if moves
                && !self.rename_pause.is_paused(&old)
                && !self.pause_and_drain(&old, &new).await
            {
                return Err(EngineError::Conflict(format!(
                    "domain '{old}' is busy with a write that has not finished, so the rename to \
                     '{new}' is waiting after its MANIFEST and link steps; {}",
                    finish_hint(&journal)
                )));
            }
            if !journal.done.contains(&step) {
                // From the index row step until the configuration is written,
                // no spelling push; the MANIFEST and relink steps of a full
                // rename come before that window.
                if moves {
                    self.names_frozen
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if step == RenameStep::Config {
                    self.rekey_in_memory(&old, &new);
                }
                let mut outcome = RenameOutcome::default();
                if let Err(e) = self
                    .run_rename_step(step, &journal, state_dir, &mut outcome)
                    .await
                {
                    return Err(self.rename_stopped(&journal, step, e));
                }
                // The relink counts go into the journal with the step, so a
                // rename finished by a resend or at the next start reports
                // them too.
                if step == RenameStep::Relink {
                    journal.relinked = Some(outcome.into_report());
                }
                journal.done.push(step);
                journal
                    .save(state_dir)
                    .map_err(|e| self.rename_stopped(&journal, step, io_error(e)))?;
            }
            // `moved` names the stores the rename moved; the MANIFEST and
            // relink steps move none and report under their own keys.
            if moves {
                moved.push(step.name());
            }
            #[cfg(any(test, feature = "testing"))]
            self.rename_checkpoint(step).await?;
        }
        self.names_frozen
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // The table and the index learn the new name and keep the old
        // spellings as aliases; references the index row step unbound (the
        // new name was another domain's spelling) bind again. Before the
        // journal goes: a crash here is finished by redoing just this.
        //
        // The new name is held under the old name's audience until the old
        // name's frame is out. A push that rebinds links announces every
        // registered domain, the new name among them, with no audience of
        // its own; a session whose cache predates the rename does not list
        // the new name as hidden, so without the hold an outsider could hear
        // a private domain's new name before the old frame marks its cache
        // stale.
        let new_held = self.capture_audience(&new, old_audience.clone());
        self.mark_names_stale();
        self.refresh_names().await;
        self.resolve_pending_everywhere().await;
        if let Err(e) = RenameJournal::remove(state_dir) {
            // Every step is done and the configuration holds the new name, so
            // the domain is served; a journal left behind is finished again,
            // harmlessly, at the next start.
            tracing::error!(
                "the rename of domain '{old}' to '{new}' is done, but its journal could not be \
                 deleted ({e}); the next start runs its last steps again"
            );
        }
        {
            let mut locks = self.origin_locks.lock().unwrap();
            locks.remove(&old);
        }
        self.rename_pause.resume(&[&old, &new]);
        // The old name's pages refetch into their not-found face and the
        // switcher picks up the new name; the old name's frame is filtered
        // against the snapshot, the new one's against each session's own
        // check, since its records were re-keyed rather than destroyed.
        self.announce_domain(&old, None, Some(old_audience));
        // Released before the new name's own frame, which keeps the ordinary
        // per-session check: the old frame has marked every cache stale.
        drop(new_held);
        self.announce_domain(&new, None, None);

        let entry = self.domain_entry(&new)?;
        if sync_after && let Err(e) = self.sync(Some(&new)).await {
            tracing::warn!(
                domain = %new,
                error = %e,
                "the sync after renaming '{old}' to '{new}' failed; the next sync catches up"
            );
        }
        let relinked = journal.relinked.clone().unwrap_or_default();
        let mut report = json!({
            "domain": new,
            "previous": old,
            "local_only": journal.local_only,
            "manifest_written": journal.done.contains(&RenameStep::Manifest)
                && !journal.manifest_kept,
            "manifest_draft": journal.manifest_draft,
            "rewritten": relinked.rewritten,
            "left_behind": relinked.left_behind,
            "aliases": entry.aliases,
            "moved": moved,
        });
        if journal.manifest_kept {
            let place = if entry.is_overlay() {
                format!("set it to '{new}' by hand in your draft of the MANIFEST.")
            } else {
                format!("set it to '{new}' by hand.")
            };
            report["note"] = json!(format!(
                "The MANIFEST's frontmatter is in a form Crystalline cannot change key by key \
                 (such as {{title: ...}}), so its domain_name was left as it is; {place}"
            ));
        }
        Ok(report)
    }

    /// Pause `old` and `new` and wait for the writes already running in
    /// `old` to finish. False, with the pause lifted again, when they do not
    /// finish in time.
    async fn pause_and_drain(&self, old: &str, new: &str) -> bool {
        self.rename_pause.pause(&[old, new]);
        #[cfg(any(test, feature = "testing"))]
        let limit = self
            .rename_drain_wait
            .lock()
            .unwrap()
            .unwrap_or(RENAME_WAIT);
        #[cfg(not(any(test, feature = "testing")))]
        let limit = RENAME_WAIT;
        if self.rename_pause.drained(old, limit).await {
            return true;
        }
        self.rename_pause.resume(&[old, new]);
        false
    }

    /// Close every co-editing room and end every join in `old`; each room
    /// saves its last text first. Whether there was anything to close.
    async fn close_editors(&self, old: &str) -> bool {
        let rooms_closed = match self.collab.get().and_then(std::sync::Weak::upgrade) {
            Some(sessions) => sessions.dispose_domain(old).await,
            None => 0,
        };
        let joins_ended = self.joins.end_domain(old);
        rooms_closed + joins_ended > 0
    }

    /// The MANIFEST step: `domain_name: <new>` into the domain's MANIFEST,
    /// through the same edit path a policy change takes, so a team domain
    /// records a local change and a reviewing domain the caller's draft.
    /// Setting the key to the value it already has changes nothing, so the
    /// step is safe to run twice.
    async fn rename_manifest(&self, journal: &RenameJournal) -> Result<()> {
        if journal.manifest_kept {
            // Even an unchanged text gets a `generated` stamp on the edit
            // path, which a flow-style frontmatter cannot take.
            return Ok(());
        }
        let scope = caller_scope(journal.caller.as_ref());
        let virtual_domain = {
            let view = DomainView::for_write(self, &journal.old, &scope).await?;
            let overlay = view.actor().map(str::to_string);
            let actor = self.actor_for(None, overlay.as_deref());
            let (desc, source) = view.resolve("manifest").await?;
            let new = journal.new.clone();
            self.apply_source_edit(
                &desc,
                &source,
                &view,
                None,
                &actor,
                None,
                &scope,
                move |current| Ok(crystalline_core::manifest::set_declared_name(current, &new)),
            )
            .await?;
            matches!(source, ContentSource::Virtual) && overlay.is_none()
        };
        // A virtual domain's declared name is cached from its MANIFEST row;
        // the cache has to carry the new one before the config step re-keys
        // it.
        if virtual_domain {
            self.refresh_routing_cache().await;
        }
        Ok(())
    }

    /// The relink step: every engram that spells one of the old names that
    /// reached this domain (the journal's `relink_spellings`) as a link's
    /// domain or in a `crystalline://` URL. In a domain the caller
    /// could write when the rename started, the spellings become the new
    /// name, through the ordinary edit path (a local change in a team domain,
    /// the caller's draft in a reviewing one). Anywhere the caller can see
    /// but not write, the engram is listed and left alone; a domain the
    /// caller cannot see is passed over without a word. A rewrite that fails
    /// is listed with its reason rather than stopping the rename, which
    /// could otherwise never finish. An engram already rewritten spells none
    /// of the old names, so the step is safe to run twice.
    async fn rename_relink(
        &self,
        journal: &RenameJournal,
        outcome: &mut RenameOutcome,
    ) -> Result<()> {
        let scope = caller_scope(journal.caller.as_ref());
        let spellings = journal.relink_spellings.as_slice();
        if spellings.is_empty() {
            return Ok(());
        }
        let found = {
            let store = self.store.lock().await;
            store.engrams_referencing_domains(spellings).await?
        };
        if found.is_empty() {
            return Ok(());
        }
        let hidden = self.hidden_for(&scope).await?;
        let new = journal.new.as_str();
        for (domain, path) in found {
            if hidden.contains(&domain) {
                continue;
            }
            if journal.writable.binary_search(&domain).is_err() {
                let references = match self.base_text(&domain, &path).await {
                    Ok(Some((_, _, text))) => respell_old(&text, spellings, new).1,
                    _ => 0,
                };
                if references > 0 {
                    outcome.left_behind.push(LeftBehind {
                        domain,
                        path,
                        references,
                        reason: None,
                    });
                }
                continue;
            }
            match self
                .relink_one(&domain, &path, &scope, spellings, new)
                .await
            {
                Ok(0) => {}
                Ok(references) => {
                    let counts = outcome.rewritten.entry(domain).or_default();
                    counts.0 += 1;
                    counts.1 += references;
                }
                Err(e) => {
                    tracing::warn!(
                        domain = domain.as_str(),
                        path = path.as_str(),
                        "the rename to '{new}' could not respell the links in this engram: {e}"
                    );
                    let references = match self.base_text(&domain, &path).await {
                        Ok(Some((_, _, text))) => respell_old(&text, spellings, new).1,
                        _ => 0,
                    };
                    outcome.left_behind.push(LeftBehind {
                        domain,
                        path,
                        references,
                        reason: Some(e.to_string()),
                    });
                }
            }
        }
        Ok(())
    }

    /// Respell one engram's links for the relink step, answering how many
    /// changed. The count is taken on the text this caller's write would
    /// change (their draft in a reviewing domain, else the domain's own), and
    /// nothing is written when it is zero.
    async fn relink_one(
        &self,
        domain: &str,
        path: &str,
        scope: &crate::scope::Scope,
        spellings: &[String],
        new: &str,
    ) -> Result<usize> {
        let view = DomainView::for_write(self, domain, scope).await?;
        let Some(desc) = self.descriptor_at(domain, path).await? else {
            return Ok(0);
        };
        let source = self.content_source(domain)?;
        let Some(text) = view.text_at(&source, &desc).await? else {
            return Ok(0);
        };
        let references = respell_old(&text, spellings, new).1;
        if references == 0 {
            return Ok(0);
        }
        let actor = self.actor_for(None, view.actor());
        self.apply_source_edit(
            &desc,
            &source,
            &view,
            None,
            &actor,
            None,
            scope,
            |current| Ok(respell_old(current, spellings, new).0),
        )
        .await?;
        Ok(references)
    }

    /// The base row at `path` in `domain`, if the index holds one.
    pub(super) async fn descriptor_at(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<Option<EngramDescriptor>> {
        let store = self.store.lock().await;
        Ok(store
            .list_engrams(domain, Some(path), None)
            .await?
            .into_iter()
            .find(|found| found.path == path))
    }

    /// The domain's own text at `path` (never a draft), with its descriptor
    /// and source, or `None` when nothing stands there.
    pub(super) async fn base_text(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<Option<(EngramDescriptor, ContentSource, String)>> {
        let Some(desc) = self.descriptor_at(domain, path).await? else {
            return Ok(None);
        };
        let source = self.content_source(domain)?;
        let text = self.load_content(&source, &desc).await?;
        Ok(Some((desc, source, text)))
    }

    /// Refuse a full rename whose MANIFEST cannot be written here, before any
    /// step, naming `--local`. A reviewing domain takes the write as the
    /// caller's draft, which counts as written: answers whether that is the
    /// case. A file domain is asked by writing and removing a file beside
    /// its MANIFEST, since permission bits do not say who may write.
    async fn refuse_unwritable_manifest(&self, old: &str, new: &str) -> Result<bool> {
        let refusal = |why: String| manifest_refusal(old, new, why);
        if self.reviews_changes(old) {
            return Ok(true);
        }
        let (desc, source) = match self.resolve_in("manifest", old).await {
            Ok(found) => found,
            Err(EngineError::NotFound(_)) => {
                return Err(refusal(format!(
                    "domain '{old}' has no MANIFEST to write the new name into"
                )));
            }
            Err(e) => return Err(e),
        };
        if let ContentSource::File { root } = &source {
            let abs = join_rel(root, &desc.path);
            let probe = crystalline_core::path::hidden_temp_path(&abs, "probe");
            match std::fs::write(&probe, b"") {
                Ok(()) => {
                    let _ = std::fs::remove_file(&probe);
                }
                Err(e) => {
                    return Err(refusal(format!(
                        "the MANIFEST of domain '{old}' cannot be written here ({}: {e})",
                        abs.display()
                    )));
                }
            }
        }
        Ok(false)
    }

    /// Whether the MANIFEST the MANIFEST step will edit can take a new name
    /// key by key: the caller's draft of it in a reviewing domain when there is
    /// one, else the base, read through the same view `rename_manifest` edits.
    /// False for a frontmatter in flow style or one that cannot be cut into
    /// keys, which the step then leaves as it is.
    async fn manifest_takes_name(
        &self,
        old: &str,
        new: &str,
        scope: &crate::scope::Scope,
    ) -> Result<bool> {
        let view = DomainView::for_write(self, old, scope).await?;
        // No MANIFEST at all, in a file or a reviewing domain alike, is the
        // refusal a file domain gets, naming `--local`.
        let (desc, source) = match view.resolve("manifest").await {
            Ok(found) => found,
            Err(EngineError::NotFound(_)) => {
                return Err(manifest_refusal(
                    old,
                    new,
                    format!("domain '{old}' has no MANIFEST to write the new name into"),
                ));
            }
            Err(e) => return Err(e),
        };
        Ok(match view.text_at(&source, &desc).await? {
            Some(text) => crystalline_core::manifest::can_declare_name(&text),
            // A draft that deletes the MANIFEST: nothing to judge here, and
            // the step answers for what it finds.
            None => true,
        })
    }

    /// Every registered domain `scope` may see and write, sorted.
    async fn writable_domains(&self, scope: &crate::scope::Scope) -> Result<Vec<String>> {
        let hidden = self.hidden_for(scope).await?;
        let mut writable = Vec::new();
        for name in self.registered_domain_entries_now().await.keys() {
            if hidden.contains(name) {
                continue;
            }
            if self.write_right(scope, name).await? >= crate::scope::DomainRight::Write {
                writable.push(name.clone());
            }
        }
        writable.sort();
        Ok(writable)
    }

    /// A step failed: the journal stays and the domain stays paused, since
    /// half its stores answer to the new name and half to the old one. The
    /// next start finishes it; so does sending the same rename
    /// again while the configuration still holds the old name.
    fn rename_stopped(
        &self,
        journal: &RenameJournal,
        step: RenameStep,
        e: EngineError,
    ) -> EngineError {
        // The spelling pushes resume for every other domain. They cannot drop
        // the old name's alias of the renamed row: until the config step no
        // configured local name finds that row, so a push never touches its
        // spellings.
        self.names_frozen
            .store(false, std::sync::atomic::Ordering::SeqCst);
        tracing::error!(
            "the rename of domain '{}' to '{}' stopped at its {} step: {e}",
            journal.old,
            journal.new,
            step.name()
        );
        EngineError::Internal(format!(
            "the rename of domain '{}' to '{}' stopped at its {} step ({e}); it finishes the \
             next time the daemon starts",
            journal.old,
            journal.new,
            step.name()
        ))
    }

    /// The test seams after each step: hold, then fail.
    #[cfg(any(test, feature = "testing"))]
    async fn rename_checkpoint(&self, step: RenameStep) -> Result<()> {
        let hold = {
            let mut armed = self.rename_hold.lock().unwrap();
            match armed.as_ref() {
                Some((at, _)) if *at == step => armed.take().map(|(_, hold)| hold),
                _ => None,
            }
        };
        if let Some(hold) = hold {
            hold.hold().await;
        }
        let fail = {
            let mut armed = self.rename_fail_after.lock().unwrap();
            if *armed == Some(step) {
                armed.take()
            } else {
                None
            }
        };
        if fail.is_some() {
            return Err(EngineError::Internal(format!(
                "the rename stopped after its {} step (test failpoint)",
                step.name()
            )));
        }
        Ok(())
    }

    /// One step, safe to run again.
    async fn run_rename_step(
        &self,
        step: RenameStep,
        journal: &RenameJournal,
        state_dir: &Path,
        outcome: &mut RenameOutcome,
    ) -> Result<()> {
        let (old, new) = (journal.old.as_str(), journal.new.as_str());
        match step {
            RenameStep::Manifest => self.rename_manifest(journal).await?,
            RenameStep::Relink => self.rename_relink(journal, outcome).await?,
            RenameStep::IndexRow => {
                let store = self.store.lock().await;
                store.rename_domain_row(old, new).await?;
            }
            RenameStep::AuthTables => {
                if let Some(access) = self.rename_access().await? {
                    access.rename_domain(old, new).await.map_err(|e| {
                        EngineError::Internal(format!("moving the records of '{old}': {e:#}"))
                    })?;
                }
            }
            RenameStep::OriginsDir => {
                move_state_dir(&self.rename_origins_root()?, old, new).map_err(io_error)?;
            }
            RenameStep::OverlaysDir => {
                move_state_dir(
                    &state_dir.join(crate::overlay_journal::JOURNAL_DIR),
                    old,
                    new,
                )
                .map_err(io_error)?;
            }
            RenameStep::ProvisionReceipt => {
                let path = state_dir.join(RECEIPT_FILE);
                if path.is_file() {
                    let internal = |e: anyhow::Error| EngineError::Internal(format!("{e:#}"));
                    let mut receipt =
                        crystalline_core::provision::receipt::load(&path).map_err(internal)?;
                    if receipt.rename_domain(old, new) {
                        crystalline_core::provision::receipt::save(&path, &receipt)
                            .map_err(internal)?;
                    }
                }
            }
            RenameStep::Config => self.rename_config(journal)?,
        }
        Ok(())
    }

    /// The Config step: `old` leaves the configuration and `new` takes its
    /// place with the same entry, its name origin recorded and every old
    /// spelling kept as an alias. Load-modify-save through `file_config` then
    /// `config`, as registration does; the table is marked stale only after
    /// both hold the new name. A configuration that already carries `new` has
    /// the same merge applied again, which changes nothing.
    fn rename_config(&self, journal: &RenameJournal) -> Result<()> {
        let (old, new) = (journal.old.as_str(), journal.new.as_str());
        let entry = {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard);
            let (index, mut entry) = match (
                file.domains.get_index_of(old),
                file.domains.get_index_of(new),
            ) {
                (Some(_), Some(_)) => {
                    return Err(EngineError::Conflict(format!(
                        "both '{old}' and '{new}' are in the configuration file; remove the \
                             one that is not this domain by hand and rerun"
                    )));
                }
                (Some(i), None) | (None, Some(i)) => {
                    let (_, entry) = file.domains.shift_remove_index(i).expect("indexed");
                    (i, entry)
                }
                (None, None) => {
                    return Err(EngineError::Internal(format!(
                        "domain '{old}' is no longer in the configuration file, so the \
                             rename to '{new}' has nothing to write"
                    )));
                }
            };
            entry.name_origin = Some(journal.origin);
            let mut aliases = entry.aliases.clone();
            aliases.extend(journal.old_spellings.iter().cloned());
            entry.aliases = distinct_without(aliases, new);
            if journal.canonical.is_some() {
                entry.canonical_seen = journal.canonical.clone();
            }
            file.domains
                .shift_insert(index, new.to_string(), entry.clone());
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
            entry
        };
        self.discovered_domains.write().unwrap().remove(old);
        // After both configurations hold the new name: a lookup that took the
        // stale mark persist set would have built from the old snapshot.
        self.mark_names_stale();
        // The mount table reads this machine's names: a domain that moved
        // off or onto a name a source gave out is shown or hidden at once.
        self.sync_sources_local();
        if let Some(tx) = &self.watch_tx
            && !entry.is_virtual()
            && let Some(root) = entry.file_path()
        {
            let _ = tx.send(WatchEvent::Remove(old.to_string()));
            let _ = tx.send(WatchEvent::Add(new.to_string(), root));
        }
        Ok(())
    }

    /// What memory keys by the old name moves to the new one before the
    /// configuration does.
    fn rekey_in_memory(&self, old: &str, new: &str) {
        {
            let mut hosted = self.hosted.write().unwrap();
            if let Some(id) = hosted.remove(old) {
                hosted.insert(new.to_string(), id);
            }
        }
        self.origin_poller.rename_domain(old, new);
        {
            let mut routing = self.routing_virtual.write().unwrap();
            if let Some(bullets) = routing.remove(old) {
                routing.insert(new.to_string(), bullets);
            }
        }
        {
            let mut declared = self.virtual_domain_names.write().unwrap();
            if let Some(name) = declared.remove(old) {
                declared.insert(new.to_string(), name);
            }
        }
    }

    /// Hold the origin lock of both names, `old` first, so no origin pull or
    /// share runs under either while the rename moves the domain.
    async fn lock_both_origins(
        &self,
        old: &str,
        new: &str,
    ) -> [tokio::sync::OwnedMutexGuard<()>; 2] {
        let first = self.origin_lock(old).lock_owned().await;
        let second = self.origin_lock(new).lock_owned().await;
        [first, second]
    }

    /// Take the one-rename slot, or say which rename holds it.
    ///
    /// `hidden` is this caller's hidden set (empty for the crash-recovery
    /// callers, which have no caller to protect against): a rename already
    /// running names its two domains in the refusal only when neither is
    /// hidden from `hidden`, for the same reason the taken-name refusal
    /// above does not name a hidden holder.
    fn take_rename_slot(
        &self,
        old: &str,
        new: &str,
        local_only: bool,
        hidden: &HashSet<String>,
    ) -> Result<RenameSlot<'_>> {
        let mut slot = self.rename_slot.lock().unwrap();
        if let Some((running, to, local)) = slot.as_ref() {
            let message = if hidden.contains(running) || hidden.contains(to) {
                "a rename is still running; try again once it is done".to_string()
            } else {
                format!(
                    "the rename of '{running}' to '{to}' is still running; try again once it is \
                     done. If it stops before it is done, {}",
                    finish_hint_for(running, to, *local)
                )
            };
            return Err(EngineError::Conflict(message));
        }
        *slot = Some((old.to_string(), new.to_string(), local_only));
        Ok(RenameSlot {
            slot: &self.rename_slot,
        })
    }

    /// Refuse while another live instance serves this index: the domain row
    /// is shared, so a rename on this machine alone would rename the domain
    /// under that instance too.
    async fn refuse_shared_index(&self, old: &str) -> Result<()> {
        let now = Utc::now();
        let stats = {
            let store = self.store.lock().await;
            store.domain_stats().await?
        };
        if let Some(row) = stats.iter().find(|row| self.hosted_elsewhere(row, now)) {
            return Err(EngineError::Conflict(format!(
                "domain '{old}' is in an index that instance {} also serves (last heartbeat {}); \
                 a rename on this machine alone would rename it under that instance too. Stop \
                 that instance first, then rename",
                row.host_instance_id.as_deref().unwrap_or_default(),
                row.host_heartbeat_at.as_deref().unwrap_or("unknown")
            )));
        }
        Ok(())
    }

    /// Refuse when state a removed domain left under the new name is in the
    /// way: an index row (removal keeps it) or a state folder. Both would
    /// otherwise stop the rename halfway or hand the old state to it.
    ///
    /// The state-folder refusal names a path on this server's filesystem,
    /// which is server-layout information a non-admin owner of a private
    /// domain (the other caller who reaches this rename) has no other way to
    /// learn; `scope` decides whether the message may say it, never whether
    /// the rename is refused - the status is the same 409 either way.
    async fn refuse_leftovers(
        &self,
        new: &str,
        state_dir: &Path,
        scope: &crate::scope::Scope,
    ) -> Result<()> {
        let row = {
            let store = self.store.lock().await;
            store.domain_id(new).await?
        };
        if row.is_some() {
            return Err(EngineError::Conflict(format!(
                "the index still holds a domain named '{new}' that is not registered here; \
                 pick another name, or run `crystalline doctor --fix`, which drops it once it \
                 holds nothing"
            )));
        }
        for parent in [
            self.rename_origins_root()?,
            state_dir.join(crate::overlay_journal::JOURNAL_DIR),
        ] {
            let taken = match std::fs::read_dir(&parent) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy() == new),
                Err(_) => false,
            };
            if taken {
                return Err(EngineError::Conflict(if may_see_server_paths(scope) {
                    format!(
                        "{} is left over from a domain once named '{new}'; move it out of the \
                         way and rename again",
                        parent.join(new).display()
                    )
                } else {
                    format!(
                        "'{new}' cannot be used as a name here; ask an instance admin to clear \
                         what a removed domain left behind under that name"
                    )
                }));
            }
        }
        Ok(())
    }

    /// The accounts database the rename moves records in: the one the HTTP
    /// surface installed, else the file under the state directory, opened for
    /// this rename alone (a daemon at startup and a standalone command have
    /// none installed). `None` when there is no accounts database at all.
    async fn rename_access(&self) -> Result<Option<Arc<crate::scope::DomainAccess>>> {
        if let Some(access) = self.domain_access.get() {
            return Ok(Some(access.clone()));
        }
        let path = self.journal_state_dir()?.join("web-auth.db");
        if !path.is_file() {
            return Ok(None);
        }
        let auth = crate::auth_store::AuthStore::open(&path)
            .await
            .map_err(|e| {
                EngineError::Internal(format!(
                    "opening the accounts database {}: {e:#}",
                    path.display()
                ))
            })?;
        Ok(Some(Arc::new(crate::scope::DomainAccess::new(Arc::new(
            auth,
        )))))
    }

    /// Where the `origins/<name>/` folders live: the test override, else the
    /// state directory's `origins`, which is where the real resolver puts
    /// them too.
    fn rename_origins_root(&self) -> Result<PathBuf> {
        match &self.origins_dir_override {
            Some(dir) => Ok(dir.clone()),
            None => Ok(self.journal_state_dir()?.join("origins")),
        }
    }

    /// The valid name `name`'s MANIFEST declares, if any.
    pub(super) fn declared_domain_name(&self, name: &str, entry: &DomainEntry) -> Option<String> {
        if entry.is_virtual() {
            return self.virtual_domain_names.read().unwrap().get(name).cloned();
        }
        entry.file_path().and_then(|root| domain_name_at(&root))
    }

    /// The refusal for a caller who may see the domain and does not own it.
    fn rename_refusal(name: &str) -> EngineError {
        EngineError::Forbidden(format!(
            "renaming domain '{name}' is for an instance admin, or for the owner of a private \
             domain; ask an admin to rename it"
        ))
    }
}

/// The provision receipt's file name under the state directory.
const RECEIPT_FILE: &str = "provisions.json";

/// The conflict a request answers when a rename kept a domain paused past the
/// wait.
fn renaming_conflict(name: String) -> EngineError {
    EngineError::Conflict(format!(
        "domain '{name}' is being renamed; try again in a moment"
    ))
}

fn io_error(e: std::io::Error) -> EngineError {
    EngineError::Internal(e.to_string())
}

/// Whether `scope` may already learn a path on this server's filesystem: the
/// machine owner (the CLI, the control socket, a local stdio MCP session)
/// always can, and so can an instance admin, who administers every domain. A
/// private domain's own non-admin owner reaches a rename too - the same
/// `require_domain_owner_refusing` gate that admits an admin admits them -
/// and a refusal built for them carries none of the server's own layout.
fn may_see_server_paths(scope: &crate::scope::Scope) -> bool {
    matches!(
        scope,
        crate::scope::Scope::Unrestricted | crate::scope::Scope::User { admin: true, .. }
    )
}

/// How to finish the rename `journal` records: the same rename sent again,
/// or a daemon restart, which finishes it before serving.
fn finish_hint(journal: &RenameJournal) -> String {
    finish_hint_for(&journal.old, &journal.new, journal.local_only)
}

/// How to finish a rename whose journal belongs to another index than the
/// one this command opened: the same command, against the index it belongs
/// to.
fn finish_elsewhere_hint(journal: &RenameJournal) -> String {
    format!(
        "run `crystalline domain rename {} {}{}` without --db and --config, or restart the \
         daemon, which finishes it before serving. When it was recorded against another \
         spelling of this machine's configuration or database, `crystalline doctor` says what \
         to put back first, and `crystalline doctor --discard-rename` drops it, leaving the \
         steps already done as they are",
        journal.old,
        journal.new,
        if journal.local_only { " --local" } else { "" }
    )
}

/// [`finish_hint`] for the rename of `old` to `new`, `--local` when
/// `local_only`.
fn finish_hint_for(old: &str, new: &str, local_only: bool) -> String {
    format!(
        "send the same rename again (`crystalline domain rename {old} {new}{}`) to finish it, \
         or restart the daemon, which finishes it before serving",
        if local_only { " --local" } else { "" }
    )
}

/// The caller a full rename's journal keeps: `None` for the machine owner.
fn rename_caller(scope: &crate::scope::Scope) -> Option<RenameCaller> {
    match scope {
        crate::scope::Scope::User { account, admin } => Some(RenameCaller {
            account: account.clone(),
            admin: *admin,
        }),
        // Anonymous never owns a domain, so it never reaches a rename.
        crate::scope::Scope::Unrestricted | crate::scope::Scope::Anonymous => None,
    }
}

/// The scope the journal's caller stands for.
fn caller_scope(caller: Option<&RenameCaller>) -> crate::scope::Scope {
    match caller {
        Some(caller) => crate::scope::Scope::User {
            account: caller.account.clone(),
            admin: caller.admin,
        },
        None => crate::scope::Scope::Unrestricted,
    }
}

/// `text` with every link domain and `crystalline://` domain that is one of
/// `spellings` respelled as `new`, and how many were.
fn respell_old(text: &str, spellings: &[String], new: &str) -> (String, usize) {
    crystalline_core::relink::respell_domains(text, &|domain: &str| {
        spellings
            .iter()
            .any(|s| s == domain)
            .then(|| new.to_string())
    })
}

/// `names` in their first order, each once, without `new`.
fn distinct_without(names: Vec<String>, new: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        if name != new && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The domains that answered to `new` before a rename of `old` gave it to
/// `old`: the one it resolved to, or every claimant of a contested canonical.
fn shadowed_by(table: &crystalline_core::names::NameTable, old: &str, new: &str) -> Vec<String> {
    let mut shadows: Vec<String> = table
        .resolve(new)
        .filter(|local| *local != old)
        .map(str::to_string)
        .into_iter()
        .collect();
    for conflict in table.conflicts() {
        if conflict.name == new {
            shadows.extend(conflict.claimants.iter().filter(|c| *c != old).cloned());
        }
    }
    shadows.sort();
    shadows.dedup();
    shadows
}

/// `'a'`, `'a' and 'b'`, `'a', 'b' and 'c'`.
fn quoted_list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// The refusal of a full rename whose MANIFEST cannot be written, naming
/// the way out: a rename on this machine only.
fn manifest_refusal(old: &str, new: &str, why: String) -> EngineError {
    EngineError::Invalid(format!(
        "{why}; to rename it on this machine only, run `crystalline domain rename {old} \
         {new} --local` or pick This machine only in Rename domain on the domain page, which \
         leaves the MANIFEST and the links as they are"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_index::TursoStore;

    /// While a rename has a domain paused, the read views' way to a domain
    /// row looks the row up and never creates one: before the index row step
    /// it finds the row under the old name, after it the old name answers a
    /// conflict instead of registering a second, empty row.
    #[tokio::test]
    async fn a_paused_domain_row_is_looked_up_never_created() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: Eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Eng\n\n## Scope\n\n- things\n\n## When to Use\n\n- routing\n",
        )
        .unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.domains
            .insert("eng".to_string(), DomainEntry::file(root.clone()));
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, None)
            .with_state_dir(tmp.path().join("state"));
        engine.sync(None).await.unwrap();
        let id = engine.store.lock().await.domain_id("eng").await.unwrap();

        engine.rename_pause.pause(&["eng", "platform"]);
        let (found, _) = engine.domain_source("eng").await.unwrap();
        assert_eq!(Some(found), id, "the row under the old name is found");

        engine
            .store
            .lock()
            .await
            .rename_domain_row("eng", "platform")
            .await
            .unwrap();
        let Err(err) = engine.domain_source("eng").await else {
            panic!("the old name answered a row after the index row moved");
        };
        assert!(
            matches!(&err, EngineError::Conflict(m) if m.contains("is being renamed")),
            "{err:?}"
        );
        assert_eq!(
            engine.store.lock().await.domain_id("eng").await.unwrap(),
            None,
            "no second row under the old name"
        );
    }
}
