use super::*;
use crate::params::Readers;
use crystalline_core::config::EffectiveShareIdentity;

/// What a connect to GitHub on an instance with a GitHub App is told when
/// it does not say who may read the domain. Access there follows GitHub, so
/// the choice is asked every time and never defaulted: nobody opens a
/// confidential repository by accident. A surface that asks the question
/// itself (the CLI prompt, the MCP refusal) matches on this text, which
/// also travels over the control socket as a plain string.
pub const READERS_REQUIRED: &str = "say who may read this domain: shared (everyone signed in) or private (only people with access on GitHub)";

/// Why a private connect is refused on an instance with no accounts store
/// open: the private record lives there, and without it the domain would be
/// open to everyone signed in the moment accounts exist.
const PRIVATE_NEEDS_ACCOUNTS: &str = "a private domain is recorded in this instance's accounts, and none are set up yet: add the first account with `crystalline users add` and restart the server, or connect it with readers shared";

/// Why a shared connect is refused on a name that is already private.
const SHARED_ON_PRIVATE: &str =
    "this domain is private; connect it as private, then change who may read on its Members card";

/// The note a connect carries when it was given readers on an instance
/// without a GitHub App.
const READERS_IGNORED: &str = "this instance has no GitHub App, so readers was ignored: who may read this domain follows this instance's own accounts, as for any other domain";

/// What a connect does about who may read the domain; see
/// [`Engine::connect_readers`].
#[derive(Clone, Copy)]
enum ConnectReaders {
    /// No App: nothing to ask, nothing given.
    Ungoverned,
    /// No App, and readers were given anyway: ignored, with a note.
    Ignored,
    /// An App: the domain will be governed, readers as chosen.
    Governed(Readers),
}

/// The MANIFEST's path in a domain and in every share of it.
const MANIFEST_PATH: &str = "MANIFEST.md";

/// Why the MANIFEST rides along on a share that adds the team marker, as
/// the share receipt and the preview name it beside the path.
pub const TEAM_MARKER_REASON: &str = "marks this repository as a team domain; added once";

/// The line a share that adds the team marker ends its pull request body or
/// commit message with, so a reviewer reads why the MANIFEST changed.
const TEAM_MARKER_NOTE: &str = "MANIFEST.md gains `team: github`. Crystalline added it: it marks this repository as a team domain, and it is added once.";

/// What the team marker does for one share: the MANIFEST text the share
/// carries, and the file list it goes on with when the marker joined a
/// narrowed one.
struct TeamMarker {
    /// The MANIFEST text before the marker, for a share that ends up
    /// sharing nothing and takes the marker back out of the folder.
    current: String,
    /// Whether the marker was written into the folder (a domain that takes
    /// changes directly) rather than into a staged tree.
    in_folder: bool,
    marked: String,
    files: Option<Vec<String>>,
}

/// The receipt and preview entry for a share that carries the team marker.
fn team_marker_json() -> Value {
    json!({ "path": MANIFEST_PATH, "reason": TEAM_MARKER_REASON })
}

/// Put the case-only moves a share will not carry on its plan or its receipt.
/// Nothing is added when there are none.
fn note_unshared_spelling(answer: &mut Value, moves: &[Value]) {
    if !moves.is_empty() {
        answer["spelling_not_shared"] = json!(moves);
        answer["spelling_note"] = json!(
            "A share does not carry a change of letter case. The team's file keeps its name, \
             and any new text lands there."
        );
    }
}

/// The text of an error a list answer carries in a row, in the words of who reads
/// it: the same list as [`plain_refusal_text`], here where the caller is known.
fn row_error(scope: &crate::scope::Scope, e: &EngineError) -> String {
    let text = e.to_string();
    let admin = matches!(
        scope,
        crate::scope::Scope::Unrestricted | crate::scope::Scope::User { admin: true, .. }
    );
    if admin {
        return text;
    }
    plain_refusal_text(&text).unwrap_or(text)
}

/// What the `connection` block of a status leaves out for a non-admin: the reason
/// names a setting, the owner's and the agent's slots are about who else this
/// instance shares as, and the App's sentences hold a command line and an
/// internal host. The mode stays, because the share surfaces branch on it.
pub fn leave_out_for_non_admin(connection: &mut Value) {
    if let Value::Object(map) = connection {
        map.remove("share_identity_reason");
        map.remove("owner_identity");
        map.remove("agent_identity");
        map.remove("app");
    }
}

/// A share refusal in the words of who reads it. A person with a session or an
/// agent over HTTP may not be an admin, so plain words and the fix. The machine
/// owner is at a command line. The App's own sentences hold a command line and a
/// host, so they go to the log here.
fn refusal_for(actor: &ShareActor, e: EngineError) -> EngineError {
    if !matches!(actor, ShareActor::Account(_) | ShareActor::HttpAgent) {
        return e;
    }
    match &e {
        EngineError::Remote(RemoteError::Refused(text)) if text == PERSONAL_LINK_MISSING => {
            RemoteError::Refused(PERSON_LINK_MISSING.to_string()).into()
        }
        EngineError::Remote(RemoteError::Refused(text)) if text == PERSONAL_TOKEN_MISSING => {
            RemoteError::Refused(PERSON_TOKEN_MISSING.to_string()).into()
        }
        EngineError::Remote(RemoteError::AppCredential(_)) => {
            tracing::warn!("a share was refused: {e}");
            RemoteError::Refused(LinkSetupReason::AppUnusable.plain_sentence().to_string()).into()
        }
        EngineError::Conflict(text) if text == github_app::KEYCHAIN_REFUSAL => {
            tracing::warn!("a share was refused: {e}");
            RemoteError::Refused(LinkSetupReason::Keychain.plain_sentence().to_string()).into()
        }
        _ => e,
    }
}

impl Engine {
    // --- origin (GitHub collaboration) ----------------------------------------

    /// Connects a new domain to a GitHub repository: downloads its tracked
    /// subtree, registers it in the global config and brings it into the
    /// index, mirroring what `domain add` does for a local folder.
    ///
    /// Without `domain`, the name is the `domain_name` the repository's
    /// MANIFEST declares (read through the forge first; stepped to
    /// `<name>-2` when taken, never adopting the holder), else the
    /// repository's own name segment; `folder` defaults to
    /// `<domains_root>/<domain>`. An explicit name is written into a MANIFEST
    /// that declares none, as a pending local change. `path` is the
    /// subfolder within the repository that is the domain root (absent means
    /// the repository root); `branch` defaults to the repository's default
    /// branch, asked from the forge and recorded in the entry.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (this both
    /// writes content and mutates config, exactly the two things read-only
    /// mode protects). A fresh connect returns `{ domain, root, engrams,
    /// base_commit, adopted, files_added, local_changes }`, so a caller knows
    /// what landed and whether existing local knowledge was adopted. A retry
    /// of the exact same connect - matching repo, subpath, branch and folder -
    /// instead returns `{ domain, root, engrams, base_commit, already_connected:
    /// true }`, so a client that timed out on the first attempt reads the
    /// connected state rather than a conflict. Both also carry the name
    /// fields every registration result does: `name_origin`,
    /// `canonical_name`, `aliases`, `shadowed` and, when shadowed, a `note`.
    ///
    /// `readers` says who may read the domain on an instance with a GitHub
    /// App ([`Engine::github_app_configured`]), where its access follows
    /// GitHub. There it is required, and refused with [`READERS_REQUIRED`]
    /// before anything is fetched when absent; `private` also needs the
    /// accounts store installed. `private` is recorded before the domain is
    /// registered, and only the acl row it wrote is taken back when the
    /// registration fails. `shared` writes and deletes nothing, and is
    /// refused before the download on a name that already carries a private
    /// record (a private domain is made shared on its Members card, never by
    /// a connect). A fresh connect then reports `readers` (`shared` or
    /// `private`). Without an App the answer is ignored and the report
    /// carries a `readers_note` saying so. An already-connected retry changes
    /// no record and reports the readers as they stand.
    pub async fn origin_add(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
        readers: Option<Readers>,
    ) -> Result<Value> {
        self.origin_add_with_progress(repo, domain, path, branch, folder, readers, None)
            .await
    }

    /// [`origin_add`](Self::origin_add) with an optional stage-boundary
    /// progress callback. A real connect reports four stages through it -
    /// downloading, downloaded, indexing, connected - so a client can keep
    /// its request timeout alive during a long download and index; an
    /// already-connected retry is instant and reports none.
    #[allow(clippy::too_many_arguments)]
    pub async fn origin_add_with_progress(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
        readers: Option<Readers>,
        progress: Option<OriginProgress>,
    ) -> Result<Value> {
        self.origin_add_with_progress_as(
            repo,
            domain,
            path,
            branch,
            folder,
            readers,
            progress,
            &crate::scope::Scope::Unrestricted,
        )
        .await
    }

    /// [`Engine::origin_add_with_progress`] for `scope`, which decides only
    /// how the report's note names a connected server: by name for the
    /// machine owner, not at all for anyone else.
    #[allow(clippy::too_many_arguments)]
    pub async fn origin_add_with_progress_as(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
        readers: Option<Readers>,
        progress: Option<OriginProgress>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let progress_at = |step: u64, msg: &str| {
            if let Some(p) = &progress {
                p(step, 4, msg);
            }
        };
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // Before anything is downloaded or origin state written: the
        // registration at the end would refuse a file that does not parse.
        self.refuse_a_config_that_does_not_parse()?;
        // Who may read it, asked before anything else is: on an instance
        // with a GitHub App the answer decides whether the content is open to
        // everyone signed in, so it is never defaulted.
        let readers = self.connect_readers(readers)?;
        // The answer as given, for a retry that finds the domain connected:
        // a different answer there is named, never applied.
        let asked = match readers {
            ConnectReaders::Governed(chosen) => Some(chosen),
            _ => None,
        };

        // The name, in order: the caller's own; else the `domain_name` the
        // repository's MANIFEST declares, read through the forge before
        // anything is downloaded; else the repository's own name. A nameless
        // connect needs the provider and the branch for that read, so it
        // resolves both here and the download below reuses them.
        let mut resolved: Option<(Arc<dyn Provider>, String)> = None;
        let (domain_name, name_origin, declared_by_manifest) = match domain {
            Some(d) => (d.to_string(), NameOrigin::Explicit, false),
            None => {
                // A retry of a nameless connect that already landed answers
                // the connected state before anything else, and without a
                // network call: the name it landed on may be a stepped one,
                // or one the MANIFEST gave, so looking the repository's
                // default name up (or re-reading the MANIFEST, whose name is
                // now taken by that very connect) would miss it and connect
                // a second copy.
                if let Some((name, entry)) =
                    self.connection_matching(repo, path, branch, folder, true)
                {
                    return self.origin_already_connected(&name, &entry, asked).await;
                }
                // The repository's own name is the fallback, and what holds it
                // here is known without asking the forge: a domain connected
                // exactly this way answers now, as it did before the MANIFEST
                // was read at all. A local refusal of it (an env-defined, a
                // virtual or a differently rooted domain) only stands when the
                // forge cannot be asked either - a MANIFEST name would make
                // it moot - and is then reported in place of the credential
                // or network error, which would send the person to the wrong
                // fix.
                let default_name = origin::default_domain_name(repo);
                let default_refusal =
                    match self.holder_of(&default_name, repo, path, branch, folder) {
                        Ok(Holder::Connected(entry)) => {
                            return self
                                .origin_already_connected(&default_name, &entry, asked)
                                .await;
                        }
                        Ok(_) => None,
                        Err(e) => Some(e),
                    };
                let provider = match self.resolve_origin_provider() {
                    Ok(p) => p,
                    Err(e) => return Err(default_refusal.unwrap_or(e)),
                };
                let branch_name = match self
                    .origin_branch_or_default(provider.as_ref(), repo, branch)
                    .await
                {
                    Ok(b) => b,
                    Err(e) => return Err(default_refusal.unwrap_or(e)),
                };
                // The branch is known now, so the exact match can be asked of
                // every domain: the same repository, subtree and branch
                // connected under a name of somebody's own choosing is this
                // checkout too, and must not be downloaded a second time
                // under a stepped name.
                if let Some((name, entry)) =
                    self.connection_matching(repo, path, Some(&branch_name), folder, false)
                {
                    return self.origin_already_connected(&name, &entry, asked).await;
                }
                let declared = self
                    .origin_manifest_name(provider.as_ref(), repo, path, &branch_name)
                    .await;
                resolved = Some((provider, branch_name));
                let entries = self.registered_domain_entries();
                let table = self.name_table_now().await;
                let choice = choose_domain_name(
                    None,
                    declared.as_deref(),
                    || origin::default_domain_name(repo),
                    |candidate| {
                        entries.contains_key(candidate)
                            || table.resolve(candidate).is_some()
                            || case_twin(candidate, None, entries.iter()).is_some()
                    },
                );
                let declared_by_manifest = declared
                    .as_deref()
                    .is_some_and(|d| validate_domain_name(d).is_ok());
                (choice.name, choice.origin, declared_by_manifest)
            }
        };
        // A name a connected server already gave out is the mount's; a new
        // local domain under it is the latecomer and gets the local suffix
        // (decision D13), before the name is checked and the default folder
        // derived from it. A registration of that name here keeps its name,
        // so adopting it in place works as before.
        let wanted = domain_name;
        let domain_name = self.beside_mounts(&wanted);
        // A name nothing holds is a new registration: check it before the
        // default folder is derived from it. A derived name passes by
        // construction (`origin::default_domain_name`, and
        // `choose_domain_name` only ever hands back a MANIFEST name, stepped
        // or not, that validates); an explicit one may not.
        if self.domain_entry(&domain_name).is_err() {
            validate_domain_name(&domain_name).map_err(EngineError::Invalid)?;
            self.refuse_case_twin(&domain_name, None, scope).await?;
        }
        // An explicit name and the repository default
        // keep adopting an origin-less domain that holds them in place, as
        // they always have; a MANIFEST-declared name that is taken has
        // already stepped to `<name>-2` above and never adopts. This one
        // flag decides both cases.
        let adopts_in_place = !declared_by_manifest;
        // A registered name is adoptable when it is an origin-less file
        // domain and the caller does not point somewhere else: the origin
        // attaches to the existing root in place and local knowledge is
        // kept. Anything else stays a conflict.
        let existing = match self.domain_entry(&domain_name) {
            Err(_) => None,
            Ok(_) if !adopts_in_place => {
                return Err(EngineError::Conflict(format!(
                    "domain '{domain_name}' is already registered; pass a domain name to connect this origin under a different one"
                )));
            }
            Ok(_) => match self.holder_of(&domain_name, repo, path, branch, folder)? {
                Holder::Free => None,
                Holder::Connected(entry) => {
                    return self
                        .origin_already_connected(&domain_name, &entry, asked)
                        .await;
                }
                Holder::Adopt(root, entry) => Some((root, entry)),
            },
        };

        let lock = self.origin_lock(&domain_name);
        let _guard = lock.lock().await;

        // Re-read the config under the lock. A connect that raced ahead of us
        // - a timed-out client's first attempt, still downloading when our
        // retry slipped past the pre-lock guard with no origin on file yet -
        // may have persisted its origin while we queued here. Answer a
        // now-matching origin idempotently instead of downloading the whole
        // repo again, and a now-conflicting one with the same conflict the
        // pre-lock guard raises. The locked helper skips the lock we hold, and
        // no progress stage fires: an idempotent return reports none, exactly
        // like the pre-lock guard's.
        if let Ok(entry) = self.domain_entry(&domain_name)
            && let Some(origin_cfg) = &entry.origin
        {
            if Self::origin_matches_request(&entry, origin_cfg, repo, path, branch, folder) {
                return self
                    .origin_already_connected_locked(&domain_name, &entry, asked)
                    .await;
            }
            return Err(EngineError::Conflict(format!(
                "domain '{domain_name}' is already connected to {}; pass a domain name to connect this origin under a different one",
                origin_cfg.repo
            )));
        }
        // A shared answer never opens a name that is already private: a
        // private domain being adopted, or the record an earlier
        // registration of the name left. Asked once the name is final and
        // before the download (a nameless connect has only read the MANIFEST
        // by now), so nothing is fetched or registered, and nothing is
        // written or deleted for it.
        if matches!(readers, ConnectReaders::Governed(Readers::Shared))
            && self.name_is_private(&domain_name).await?
        {
            return Err(EngineError::Invalid(SHARED_ON_PRIVATE.to_string()));
        }

        let adopts_registered = existing.is_some();
        if !adopts_registered {
            self.set_aside_orphan_kept(&domain_name);
        }
        let (root, adopted_entry) = match existing {
            Some((r, entry)) => (r, Some(entry)),
            None => (
                match folder {
                    Some(f) => crystalline_core::config::expand_tilde(f),
                    None => {
                        let domains_root = self.config.read().unwrap().domains_root();
                        origin::default_domain_folder(&domains_root, &domain_name)
                    }
                },
                None,
            ),
        };
        let (provider, branch_name) = match resolved {
            Some(both) => both,
            None => {
                let provider = self.resolve_origin_provider()?;
                let branch_name = self
                    .origin_branch_or_default(provider.as_ref(), repo, branch)
                    .await?;
                (provider, branch_name)
            }
        };
        let spec = OriginSpec {
            repo: repo.to_string(),
            subpath: path.map(str::to_string),
            branch: branch_name.clone(),
        };
        let state_dir = self.origin_state_dir(&domain_name)?;
        progress_at(1, &format!("downloading {repo}"));
        let report = ops::subscribe(
            provider.as_ref(),
            &spec,
            &root,
            &state_dir,
            subscribed_reflection_mode,
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))?;
        progress_at(
            2,
            &format!(
                "downloaded {} engrams, registering the domain",
                report.engrams
            ),
        );

        // A private domain is recorded private before it is registered, so
        // there is no moment in which the registered domain is governed and
        // open to everyone signed in. Should the registration below fail,
        // the record is taken back again.
        let closed = match readers {
            ConnectReaders::Governed(Readers::Private) => {
                Some(self.record_private_readers(&domain_name, scope).await?)
            }
            _ => None,
        };

        // Register the domain and persist, mirroring `configure`'s file-then-
        // effective write-lock-first pattern so a concurrent read never observes
        // a half-applied config and no env value bakes into the saved file.
        let registered = (|| -> Result<()> {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard)?;
            // Adopting a registered domain keeps everything already decided
            // about it - whether its artifacts are provisioned, whether it
            // reviews changes, the aliases it answers to and the canonical
            // name it last saw - and gains the origin. Built from the file
            // when it holds the entry, else from the entry this call adopted.
            let mut entry = file
                .domains
                .get(&domain_name)
                .or(adopted_entry.as_ref())
                .cloned()
                .unwrap_or_default();
            entry.kind = CoreDomainKind::File;
            entry.path = Some(root.clone());
            entry.origin = Some(OriginConfig {
                repo: repo.to_string(),
                path: path.map(str::to_string),
                branch: Some(branch_name.clone()),
                poll_secs: None,
            });
            // A name the caller gave is `explicit`. A nameless connect that
            // adopted a registration keeps what that registration already
            // says: a name somebody chose on purpose stays `explicit`, so it
            // never becomes eligible for an automatic rename. Only a fresh
            // nameless registration (or a legacy entry that says nothing)
            // records `derived`. Nothing is written back into the team's
            // MANIFEST either way: a team domain never gets `domain_name`
            // written automatically.
            entry.name_origin = match name_origin {
                NameOrigin::Explicit => Some(NameOrigin::Explicit),
                NameOrigin::Derived => entry.name_origin.or(Some(NameOrigin::Derived)),
            };
            file.domains.insert(domain_name.clone(), entry);
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
            Ok(())
        })();
        if let Err(e) = registered {
            if closed == Some(crystalline_identity::auth_store::VisibilityWrite::Written) {
                self.take_back_private_readers(&domain_name).await;
            }
            return Err(e);
        }
        // Who the repository is on GitHub, so a later transfer or a reused
        // name is told apart from a rename (still under the origin lock).
        self.record_repository_ids_at_connect(&domain_name, repo, &state_dir)
            .await;
        let readers_note = match readers {
            ConnectReaders::Ignored => Some(READERS_IGNORED),
            _ => None,
        };
        // Its origin may make it a copy of a mounted domain, hidden from now on.
        self.sync_sources_local();

        // Tell a running daemon's watcher to start watching the new root; it
        // also runs its own catch-up sync and embed once the watch is armed.
        // This engine's own sync just below runs regardless, so the domain is
        // searchable immediately even outside a daemon (a standalone CLI
        // command, or a race with the watcher's async catch-up); sync is
        // checksum idempotent, so the watcher repeating it moments later is a
        // harmless no-op. An adopted registered domain is already watched.
        if !adopts_registered && let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(domain_name.clone(), root.clone()));
        }

        progress_at(3, "indexing for search");
        self.sync(Some(&domain_name)).await?;
        self.refresh_names().await;
        // A reference elsewhere that already named this domain binds now.
        self.resolve_references_into(&domain_name).await;
        // Embedding a whole freshly connected repo can outlast any client
        // timeout, so a daemon or in-process MCP server runs it on the embed
        // worker; without a worker (standalone one-shot commands, tests) the
        // inline pass keeps the old behavior, and is a no-op anyway whenever
        // no provider is loaded.
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after connecting '{domain_name}' failed: {e}");
        }

        progress_at(4, "connected");
        let mut result = json!({
            "domain": domain_name,
            "root": root.display().to_string(),
            "engrams": report.engrams,
            "base_commit": report.base_commit,
            "adopted": report.adopted || adopts_registered,
            "files_added": report.files_written,
            "local_changes": report.local_changes,
        });
        if let ConnectReaders::Governed(chosen) = readers {
            result["readers"] = json!(chosen.as_str());
        }
        if let Some(note) = readers_note {
            result["readers_note"] = json!(note);
        }
        self.append_name_fields(&mut result, &domain_name).await?;
        self.note_beside_mounts(&mut result, &wanted, &domain_name, scope);
        Ok(result)
    }

    /// What a connect does about who may read the domain, decided before
    /// anything is fetched: on an instance with a GitHub App (usable or not,
    /// see [`Engine::github_app_configured`]) the caller has to say, and a
    /// private answer needs the accounts store to record it in; without an
    /// App the answer is not asked and, when given, ignored with a note.
    fn connect_readers(&self, readers: Option<Readers>) -> Result<ConnectReaders> {
        if !self.github_app_configured() {
            return Ok(match readers {
                Some(_) => ConnectReaders::Ignored,
                None => ConnectReaders::Ungoverned,
            });
        }
        match readers {
            None => Err(EngineError::Invalid(READERS_REQUIRED.to_string())),
            Some(Readers::Private) if self.domain_access.get().is_none() => {
                Err(EngineError::Invalid(PRIVATE_NEEDS_ACCOUNTS.to_string()))
            }
            Some(chosen) => Ok(ConnectReaders::Governed(chosen)),
        }
    }

    /// Whether `domain` carries a private record, registered or not. `false`
    /// without an accounts store, where no record can exist. A record that
    /// cannot be read refuses: the answer decides whether a shared connect
    /// would open it.
    async fn name_is_private(&self, domain: &str) -> Result<bool> {
        let Some(access) = self.domain_access.get() else {
            return Ok(false);
        };
        access.has_private_record(domain).await.map_err(|e| {
            EngineError::Internal(format!(
                "whether domain '{domain}' is private could not be read, so it was not connected: {e:#}"
            ))
        })
    }

    /// Who reads a governed domain as it stands: `private` with a private
    /// record, else `shared`. `None` without an App, where the question is
    /// not asked, or when the record cannot be read.
    async fn governed_readers_now(&self, domain: &str) -> Option<Readers> {
        if !self.github_app_configured() {
            return None;
        }
        match self.name_is_private(domain).await {
            Ok(true) => Some(Readers::Private),
            Ok(false) => Some(Readers::Shared),
            Err(_) => None,
        }
    }

    /// Records `domain` private for a governed connect, naming who chose
    /// it: the signed-in account, else the machine owner's reserved name.
    async fn record_private_readers(
        &self,
        domain: &str,
        scope: &crate::scope::Scope,
    ) -> Result<crystalline_identity::auth_store::VisibilityWrite> {
        let set_by = match scope {
            crate::scope::Scope::User { account, .. } | crate::scope::Scope::Agent { account } => {
                account.as_str()
            }
            crate::scope::Scope::Unrestricted | crate::scope::Scope::Anonymous => {
                crate::scope::OWNER_IDENTITY_NAME
            }
        };
        let access = self
            .domain_access
            .get()
            .ok_or_else(|| EngineError::Invalid(PRIVATE_NEEDS_ACCOUNTS.to_string()))?;
        access
            .set_governed_private(domain, set_by)
            .await
            .map_err(|e| {
                EngineError::Internal(format!(
                    "domain '{domain}' could not be made private, so it was not connected: {e:#}"
                ))
            })
    }

    /// Takes back the private record [`Engine::record_private_readers`]
    /// wrote for a connect whose registration failed: the acl row only, never
    /// a membership. Best effort: a record left behind keeps the name
    /// private, which closes rather than opens.
    async fn take_back_private_readers(&self, domain: &str) {
        let Some(access) = self.domain_access.get() else {
            return;
        };
        if let Err(e) = access.take_back_governed_private(domain).await {
            tracing::warn!(
                "domain '{domain}' was not connected, but its private record could not be taken back: {e:#}"
            );
        }
    }

    /// Whether the share of `files` (`None` is the whole unshared delta) out
    /// of the tree at `tree` carries the team marker
    /// ([`crystalline_core::TEAM_KEY`]), and if so the MANIFEST text it
    /// carries and the file list it goes on with. `tree` is the folder for a
    /// domain that takes changes directly, and the staged tree of the
    /// actor's drafts for one that reviews changes. Reads only: the share
    /// writes what this answers, and the preview reports it.
    ///
    /// The share, not the connect, is the one place every connected instance
    /// passes through on its way to the team, existing connections included.
    /// A connect never writes into the team's MANIFEST (a fresh connect has
    /// no local change, and review mode needs exactly that clean folder to
    /// start), and a write on every pull would be a local change nobody made
    /// that comes back after every discard. At the share, the marker and the
    /// share are one act, the receipt, the preview and the pull request body
    /// say so, and on a domain that reviews changes it goes through the
    /// review like any other line:
    ///
    /// - a share of everything carries it when there is something else to
    ///   share, so pressing share with nothing to share stays a no-op;
    /// - a narrowed share carries it when the list names the MANIFEST, and
    ///   when the MANIFEST holds no other unshared change, in which case the
    ///   MANIFEST joins the list and exactly one line rides along; a MANIFEST
    ///   with unshared edits that were not chosen is left for a share that
    ///   includes it.
    ///
    /// Nothing is carried from a read-only instance or domain, from a
    /// MANIFEST that already declares a `team` (an unknown value is left for
    /// its owner: verify reports it as `M010`, and a newer release may know
    /// it) or from a frontmatter the marker cannot be cut into.
    async fn plan_team_marker(
        &self,
        domain: &str,
        tree: &Path,
        state_dir: &Path,
        files: Option<&[String]>,
    ) -> Result<Option<TeamMarker>> {
        if self.read_only || self.writes_refused(domain).await {
            return Ok(None);
        }
        let Ok(current) = std::fs::read_to_string(tree.join(MANIFEST_PATH)) else {
            return Ok(None);
        };
        let current = crystalline_core::to_lf(&current);
        let declares_team = crystalline_core::parse_engram(&current)
            .map(|e| {
                crystalline_core::Manifest::from_engram(&e, &current)
                    .declared_team()
                    .is_some()
            })
            .unwrap_or(true);
        let marked =
            crystalline_core::set_team_marker(&current, crystalline_core::TeamForge::GitHub);
        if declares_team || marked == current {
            return Ok(None);
        }
        let Some(state) = crystalline_remote::state::OriginState::load(state_dir)? else {
            return Ok(None);
        };
        let local = crystalline_remote::changes::detect_local_changes_against(
            tree,
            &ops::unshared_base(&state),
            state_dir,
            self.reflection_mode(domain).await?,
        )?;
        let changed: Vec<&str> = local
            .changes
            .iter()
            .filter(|change| !change.is_generated_index())
            .map(|change| change.path())
            .collect();
        let files = match files {
            None if changed.is_empty() => return Ok(None),
            None => None,
            Some(list) if list.iter().any(|path| path == MANIFEST_PATH) => None,
            Some(_) if changed.contains(&MANIFEST_PATH) => return Ok(None),
            Some(list) => {
                let mut list = list.to_vec();
                list.push(MANIFEST_PATH.to_string());
                Some(list)
            }
        };
        Ok(Some(TeamMarker {
            current: current.to_string(),
            in_folder: false,
            marked,
            files,
        }))
    }

    /// Write the marker [`Engine::plan_team_marker`] planned into the folder
    /// of a domain that takes changes directly, just before its share.
    /// Answers whether it wrote: a MANIFEST open in a co-editing room is
    /// left to the room, and the next share tries again.
    ///
    /// Written as bytes, not through the edit path, so it carries no
    /// `generated` stamp: every instance writes exactly the same line at the
    /// same place, and a member whose share pulls the MANIFEST another
    /// member's share already marked meets a merge with nothing to
    /// reconcile. A stamp of its own would differ on every instance and make
    /// that pull a conflict nobody caused.
    async fn write_team_marker(&self, domain: &str, root: &Path, marked: &str) -> Result<bool> {
        // The write ticket and the read-only screen, as every write has.
        let scope = crate::scope::Scope::Unrestricted;
        let view = DomainView::for_write(self, domain, &scope).await?;
        let (desc, _) = view.resolve("manifest").await?;
        if let Some(rooms) = self.collab_rooms()
            && rooms
                .live_text(&desc.domain, &desc.permalink, None)
                .await
                .is_some()
        {
            return Ok(false);
        }
        write_file(&join_rel(root, &desc.path), marked)?;
        drop(view);
        self.sync(Some(domain)).await?;
        Ok(true)
    }

    /// The branch a connect tracks: the one asked for, else the one the forge
    /// calls the repository's default. Never a silent `main`: a repository
    /// whose default is `trunk` would track a branch that does not exist. A
    /// failed lookup refuses the connect.
    async fn origin_branch_or_default(
        &self,
        provider: &dyn Provider,
        repo: &str,
        branch: Option<&str>,
    ) -> Result<String> {
        match branch {
            Some(b) => Ok(b.to_string()),
            None => Ok(provider
                .default_branch(repo)
                .await
                .inspect_err(|e| self.drop_github_credential_on_auth(e))
                .map_err(|e| default_branch_refusal(repo, e))?),
        }
    }

    /// The `domain_name` the MANIFEST at the root of the connected subtree
    /// declares on `branch`, read through the forge before anything is
    /// downloaded, or `None` when it declares none, is missing or cannot be
    /// read. A failed read is only logged: the connect goes on under the
    /// repository's own name, and the download that follows reports any real
    /// problem with the repository in its own words.
    async fn origin_manifest_name(
        &self,
        provider: &dyn Provider,
        repo: &str,
        subpath: Option<&str>,
        branch: &str,
    ) -> Option<String> {
        let file = match subpath
            .map(|s| s.trim_matches('/'))
            .filter(|s| !s.is_empty())
        {
            Some(sub) => format!("{sub}/MANIFEST.md"),
            None => "MANIFEST.md".to_string(),
        };
        let spec = OriginSpec {
            repo: repo.to_string(),
            subpath: subpath.map(str::to_string),
            branch: branch.to_string(),
        };
        match provider.read_file(&spec, branch, &file).await {
            Ok(Some(bytes)) => String::from_utf8(bytes)
                .ok()
                .and_then(|text| domain_name_of_source(&text)),
            Ok(None) => None,
            Err(e) => {
                self.drop_github_credential_on_auth(&e);
                tracing::warn!(
                    repo = %repo,
                    file = %file,
                    error = %e,
                    "reading the MANIFEST before connecting failed; naming the domain after the repository"
                );
                None
            }
        }
    }

    /// Peeks at the `domain_name` a repository's MANIFEST declares, without
    /// registering anything: `{ domain_name, default_name }`. `domain_name`
    /// is `None` when the MANIFEST at `path` (the repository root when
    /// `path` is `None`) on `branch` (the repository's default when
    /// `branch` is `None`) declares none, is missing or cannot be read;
    /// `default_name` is the repository's own name segment, the fallback a
    /// nameless [`Engine::origin_add`] lands on when the MANIFEST declares
    /// none either.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with the credential error a caller who has not connected GitHub
    /// answers - a surface that wants the friendlier settings-screen wording
    /// asks [`Engine::github_ready`] first, exactly as a nameless
    /// [`Engine::origin_add`] does.
    pub async fn github_domain_name_preview(
        &self,
        repo: &str,
        path: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        let provider = self.resolve_origin_provider()?;
        let branch_name = self
            .origin_branch_or_default(provider.as_ref(), repo, branch)
            .await?;
        let domain_name = self
            .origin_manifest_name(provider.as_ref(), repo, path, &branch_name)
            .await;
        Ok(json!({
            "domain_name": domain_name,
            "default_name": origin::default_domain_name(repo),
        }))
    }

    /// What a connect finds holding `name`, with every local refusal decided
    /// here and no network call: an env-defined domain (named by its
    /// variable), a domain connected to something else, a virtual domain, and
    /// an origin-less one rooted somewhere other than the `folder` asked for
    /// are conflicts. A domain connected exactly the way this request asks is
    /// [`Holder::Connected`]; an origin-less file domain is adoptable in place.
    fn holder_of(
        &self,
        name: &str,
        repo: &str,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Holder> {
        let Ok(entry) = self.domain_entry(name) else {
            return Ok(Holder::Free);
        };
        // An env-defined domain names the variable that owns it, so the
        // operator knows to unset it rather than pick another name.
        if let Some(env) = self.overlay.env_domain(name) {
            return Err(EngineError::Conflict(format!(
                "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                env.var
            )));
        }
        if let Some(origin_cfg) = &entry.origin {
            // A retry of the exact connect that already succeeded answers with
            // the connected state instead of a conflict, so a client that timed
            // out waiting for the first response never reads success as
            // failure. This pre-lock guard keeps the common
            // retry-after-completion case instant and lock-free; a re-read
            // under the lock in `origin_add_with_progress` catches a retry that
            // raced an in-flight connect.
            if Self::origin_matches_request(&entry, origin_cfg, repo, path, branch, folder) {
                return Ok(Holder::Connected(entry));
            }
            return Err(EngineError::Conflict(format!(
                "domain '{name}' is already connected to {}; pass a domain name to connect this origin under a different one",
                origin_cfg.repo
            )));
        }
        let Some(registered_root) = entry.file_path() else {
            return Err(EngineError::Conflict(format!(
                "domain '{name}' is a virtual domain; an origin connects a file domain, so pass a different domain name"
            )));
        };
        if let Some(f) = folder {
            let wanted = crystalline_core::config::expand_tilde(f);
            if wanted != registered_root {
                return Err(EngineError::Conflict(format!(
                    "domain '{name}' is rooted at {}; omit the folder to connect it in place, or pass a different domain name",
                    registered_root.display()
                )));
            }
        }
        Ok(Holder::Adopt(registered_root, entry))
    }

    /// A registered domain already connected the way this request asks,
    /// judged the way a retry is everywhere else
    /// ([`origin_matches_request`](Self::origin_matches_request)), so a
    /// nameless connect never downloads a second checkout of what is already
    /// here, whatever name the first one landed on.
    ///
    /// Asked twice by a nameless connect. Before any network call, with the
    /// branch as the caller gave it, `derived_only` is set: that match is
    /// loose (no branch asked for matches any, no folder matches any root),
    /// and only a domain an earlier nameless connect made may answer it, so a
    /// domain somebody connected under a name of their own, on another
    /// branch, does not. Once the branch is resolved the match is exact, and
    /// any domain answers, whatever its `name_origin`: the same repository,
    /// subtree and branch connected under an explicit name is still this
    /// checkout.
    fn connection_matching(
        &self,
        repo: &str,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
        derived_only: bool,
    ) -> Option<(String, DomainEntry)> {
        self.registered_domain_entries()
            .into_iter()
            .find(|(name, entry)| {
                let eligible = !derived_only
                    || entry.name_origin.unwrap_or_else(|| {
                        infer_name_origin(name, entry, self.overlay.env_domain(name).is_some())
                    }) == NameOrigin::Derived;
                eligible
                    && entry.origin.as_ref().is_some_and(|origin_cfg| {
                        Self::origin_matches_request(entry, origin_cfg, repo, path, branch, folder)
                    })
            })
    }

    /// Whether a registered domain's origin matches this connect request
    /// exactly, so a retry answers idempotently instead of re-connecting.
    /// GitHub treats owner/name case insensitively, so the repo compares that
    /// way; the subpath compares exactly, an absent requested branch matches
    /// any stored one, and an absent stored branch means main; an omitted
    /// folder always matches, a given one must resolve to the registered root.
    /// Shared by the pre-lock guard and the re-read under the lock so both
    /// sites judge a match identically.
    fn origin_matches_request(
        entry: &DomainEntry,
        origin_cfg: &OriginConfig,
        repo: &str,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
    ) -> bool {
        let same_repo = origin_cfg.repo.eq_ignore_ascii_case(repo);
        let same_path = origin_cfg.path.as_deref() == path;
        // No branch asked for matches whatever the entry tracks: a connect
        // without one records the repository default it resolved, and a retry
        // of that connect is the same request. A stored entry with no branch
        // still means main, since it was written under that rule.
        let same_branch = match branch {
            None => true,
            Some(b) => origin_cfg.branch() == b,
        };
        let same_folder = match (folder, entry.file_path()) {
            (None, _) => true,
            (Some(f), Some(r)) => crystalline_core::config::expand_tilde(f) == r,
            (Some(_), None) => false,
        };
        same_repo && same_path && same_branch && same_folder
    }

    /// The response for a connect retry that matches the existing
    /// connection: the same shape `origin_add` returns, marked
    /// `already_connected`, read under the domain's origin lock.
    async fn origin_already_connected(
        &self,
        name: &str,
        entry: &DomainEntry,
        asked: Option<Readers>,
    ) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        self.origin_already_connected_locked(name, entry, asked)
            .await
    }

    /// [`origin_already_connected`](Self::origin_already_connected)'s body,
    /// assuming the caller already holds the domain's origin lock. The re-read
    /// inside `origin_add_with_progress` calls this directly: the origin lock
    /// is a non-reentrant tokio mutex, so re-acquiring it there would
    /// deadlock.
    ///
    /// `asked` is the readers answer the retry gave on an instance with a
    /// GitHub App: one that differs from the domain's own is not applied,
    /// and the report carries a `readers_note` saying where to change it.
    async fn origin_already_connected_locked(
        &self,
        name: &str,
        entry: &DomainEntry,
        asked: Option<Readers>,
    ) -> Result<Value> {
        let root = entry.file_path().unwrap_or_default();
        let state_dir = self.origin_state_dir(name)?;
        let base_commit = crystalline_remote::state::OriginState::load(&state_dir)?
            .map(|s| s.base_commit)
            .unwrap_or_default();
        let engrams = {
            let store = self.store.lock().await;
            store
                .domain_stats()
                .await
                .unwrap_or_default()
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.engrams)
                .unwrap_or(0)
        };
        let mut result = json!({
            "domain": name,
            "root": root.display().to_string(),
            "engrams": engrams,
            "base_commit": base_commit,
            "already_connected": true,
        });
        // Who reads it, as it stands: a retry changes no record, whatever
        // readers it was given.
        if let Some(readers) = self.governed_readers_now(name).await {
            result["readers"] = json!(readers.as_str());
            if asked.is_some_and(|asked| asked != readers) {
                result["readers_note"] = json!(format!(
                    "this domain is already connected and its readers are {}; change who may read on its Members card",
                    readers.as_str()
                ));
            }
        }
        self.append_name_fields(&mut result, name).await?;
        Ok(result)
    }

    /// Brings one origin-connected domain (or every one, when `domain` is
    /// `None`) up to date with its origin. Errors when a named domain is not
    /// registered or has no origin; one domain failing (offline, revoked)
    /// never aborts the others, each per-domain failure is collected into the
    /// `errors` array instead. Allowed on a read-only instance: a pull is a
    /// derived-truth update like sync, not a user-authored content write.
    /// Ends with [`Engine::adopt_domain_names`] once every pull has finished;
    /// the report is the pulls' own.
    pub async fn origin_update(
        &self,
        domain: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        let hidden = self.hidden_for(scope).await?;
        let targets = self.origin_targets(domain, &hidden)?;
        // Which rename questions were already respelled here before the
        // pull: the pull settles every other one its files settled.
        let clean_before = self.rename_questions_already_clean().await;

        let mut domains = Vec::new();
        let mut errors = Vec::new();
        for (name, entry) in targets {
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => domains.push(v),
                Err(e) => errors.push(json!({ "domain": name, "error": row_error(scope, &e) })),
            }
        }
        // Once every pull has finished and let go of its origin lock, which a
        // rename takes: a pulled MANIFEST may declare a new name.
        self.adopt_domain_names_after("the update").await;
        self.settle_rename_questions_after_pull(&clean_before).await;
        Ok(json!({ "domains": domains, "errors": errors }))
    }

    /// Pulls and syncs one domain, under its origin lock. The per-domain body
    /// behind `origin_update`'s aggregate loop.
    pub(super) async fn origin_update_one(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        // A pull writes the domain's files: only its host pulls, before any
        // file lands. A lock nobody holds, or whose holder went silent, is
        // claimed as a sync claims it.
        self.require_file_host(name, &crate::scope::Scope::Unrestricted)
            .await?;

        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;

        // An env-defined team domain with no origin state yet bootstraps itself
        // on first contact: the zero-config read-only node's first pull is a
        // subscribe, not an update. This is gated on the domain being
        // env-defined so a non-env domain with missing state still fails exactly
        // as before (it was never fully connected). Bootstrapping is a
        // derived-truth pull, so it is allowed on a read-only instance. The
        // env check comes first so ordinary domains skip the state read on
        // every poll tick.
        if self.overlay.env_domain(name).is_some()
            && crystalline_remote::state::OriginState::load(&state_dir)
                .ok()
                .flatten()
                .is_none()
        {
            return self
                .bootstrap_env_origin(name, &spec, &root, &state_dir)
                .await;
        }

        let provider = self.resolve_origin_provider()?;
        let report = ops::pull(provider.as_ref(), &spec, &root, &state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;

        self.sync(Some(name)).await?;
        // The pull advanced the base, so every draft standing over this domain
        // is asked whether the folder has caught up with it. After the sync,
        // not before: the pass reads the base snapshot for content but the rest
        // of this call expects a domain whose rows are current. A failure here
        // is a warning rather than a failed update - the pull has already
        // landed on disk, and the next pull runs the whole pass again.
        if !report.up_to_date
            && let Err(e) = self.converge_pulled_overlays(name, &report.applied).await
        {
            tracing::warn!("converging the drafts in '{name}' after updating failed: {e}");
        }
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after updating '{name}' failed: {e}");
        }

        // `ops::pull` already saved the post-pull state to `state_dir`; reload
        // it fresh so each transition's url and title can be joined in for
        // the caller. A reload failure only degrades the proposal entries to
        // number and status (see `origin::proposal_transitions_json`), it
        // never fails an update that has already landed on disk.
        let state = crystalline_remote::state::OriginState::load(&state_dir)
            .ok()
            .flatten();
        let proposals = origin::proposal_transitions_json(&report.proposals, state.as_ref());
        // Every still-open proposal rides along in full, review feedback
        // included: an update is where a pull refreshes it, so this is the
        // channel an agent reads reviewer comments from without a second call.
        let open_proposals: Vec<Value> = state
            .as_ref()
            .map(|s| {
                s.proposals
                    .iter()
                    .filter(|p| p.status == crystalline_remote::state::ProposalStatus::Open)
                    .map(|p| serde_json::to_value(p).expect("a proposal serializes"))
                    .collect()
            })
            .unwrap_or_default();
        let mut v = origin::pull_report_json(name, &report, proposals);
        v["open_proposals"] = Value::Array(open_proposals);
        Ok(v)
    }

    /// Bootstraps an env-defined team domain on its first contact with GitHub:
    /// creates the root, runs the same [`ops::subscribe`] `origin_add` uses
    /// (minus the config write, since an env domain is never persisted), then
    /// syncs and best-effort embeds. Called under the domain's origin lock by
    /// [`Engine::origin_update_one`]. The report is shaped like a normal update
    /// (`up_to_date`, `applied`, `merged`, `conflicts`, `proposals`) so
    /// `print_origin_update` and the poller's outcome handling keep working
    /// unchanged, plus `bootstrapped: true` and the subscribe facts (`engrams`,
    /// `base_commit`) a bootstrapped line reads from.
    async fn bootstrap_env_origin(
        &self,
        name: &str,
        spec: &OriginSpec,
        root: &Path,
        state_dir: &Path,
    ) -> Result<Value> {
        let provider = self.resolve_origin_provider()?;
        // notify refuses to watch a missing directory; the daemon pre-creates
        // env-domain roots at startup, but a subscribe run outside that path
        // (an on-demand `origin update`, a poll tick) creates it here too.
        std::fs::create_dir_all(root).map_err(|e| {
            EngineError::Internal(format!(
                "could not create the domain root {}: {e}",
                root.display()
            ))
        })?;
        let report = ops::subscribe(
            provider.as_ref(),
            spec,
            root,
            state_dir,
            subscribed_reflection_mode,
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))?;

        // Tell a running daemon's watcher to start watching the freshly
        // bootstrapped root, the same signal `origin_add` sends.
        if let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(name.to_string(), root.to_path_buf()));
        }

        self.sync(Some(name)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after bootstrapping '{name}' failed: {e}");
        }

        Ok(json!({
            "domain": name,
            "bootstrapped": true,
            "up_to_date": false,
            "applied": [],
            "merged": [],
            "conflicts": [],
            "proposals": [],
            "skipped_large": report.skipped_large,
            "re_baselined": false,
            "engrams": report.engrams,
            "base_commit": report.base_commit,
        }))
    }

    /// Records who may read every env-defined team domain, from its
    /// `CRYSTALLINE_DOMAIN_<NAME>_READERS` variable, on an instance with a
    /// GitHub App. The daemon calls this once at startup, after the accounts
    /// store is installed and before anything is served, so a domain is
    /// closed before its first request.
    ///
    /// It only ever closes: `private`, or an absent variable, writes the
    /// private record (left as it stands when there is one); `shared` writes
    /// nothing, and on a domain that is already private it says so in one
    /// warning and leaves it private, since who may read a private domain is
    /// changed on its Members card. An absent variable reads as private,
    /// with one warning naming it, since nobody should open a confidential
    /// repository by accident. Applied whether or not `github.enabled` is on,
    /// because closing a domain is never wrong.
    ///
    /// Without an App it writes nothing, and a variable set anyway is named
    /// in an info line. Without an accounts store there is no record to
    /// write, and one warning says so for every domain that should be
    /// private. A private record that cannot be written stops the start,
    /// naming the domain.
    pub async fn apply_env_readers(&self) -> Result<()> {
        self.apply_env_readers_with(false).await
    }

    /// [`Engine::apply_env_readers`] for the standalone stdio and one-shot
    /// stacks: the private records are written the same way, and nothing is
    /// said, since the daemon's start already says it once.
    pub async fn apply_env_readers_quietly(&self) -> Result<()> {
        self.apply_env_readers_with(true).await
    }

    async fn apply_env_readers_with(&self, quiet: bool) -> Result<()> {
        let targets: Vec<(String, Option<Readers>, String)> = self
            .overlay
            .env_domains()
            .filter(|(_, env)| env.entry.origin.is_some())
            .map(|(name, env)| (name.clone(), env.readers, env.readers_var()))
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        if !self.github_app_configured() {
            if !quiet {
                for (_, readers, var) in &targets {
                    if readers.is_some() {
                        tracing::info!(
                            "{var} changes nothing on an instance without a GitHub App: who may read the domain follows this instance's own accounts"
                        );
                    }
                }
            }
            return Ok(());
        }
        let access = self.domain_access.get();
        let mut unrecorded: Vec<String> = Vec::new();
        for (name, readers, var) in targets {
            if readers.is_none() && !quiet {
                tracing::warn!(
                    "{var} is not set, so the env-defined team domain '{name}' is private: only people with access on GitHub read it. Set {var} to shared or private to say who may read it"
                );
            }
            let Some(access) = access else {
                if readers != Some(Readers::Shared) {
                    unrecorded.push(name);
                }
                continue;
            };
            // Read first: a domain that is already private needs no write,
            // and a start that finds it so takes no write lock on the
            // accounts database.
            let private = access.has_private_record(&name).await.map_err(|e| {
                EngineError::Internal(format!(
                    "whether the env-defined team domain '{name}' is private could not be read: {e:#}"
                ))
            })?;
            if readers == Some(Readers::Shared) {
                // Shared never opens a domain that is already private.
                if private && !quiet {
                    tracing::warn!(
                        "{var} says shared, but domain '{name}' is private; set the variable to private, or remove the private record with: crystalline domain visibility {name} default"
                    );
                }
                continue;
            }
            if private {
                continue;
            }
            access
                .set_governed_private(&name, crate::scope::OWNER_IDENTITY_NAME)
                .await
                .map_err(|e| {
                    EngineError::Internal(format!(
                        "the env-defined team domain '{name}' could not be made private: {e:#}"
                    ))
                })?;
        }
        if !unrecorded.is_empty() && !quiet {
            tracing::warn!(
                "the env-defined team domains {} should be private, but this instance has no accounts yet to record it in; restart the server after adding the first account",
                unrecorded
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }

    /// Bootstraps every env-defined team domain that carries an origin but
    /// has no local origin state yet, bringing each up through
    /// `Engine::origin_update_one` so bootstrapping and a plain background
    /// pull stay exactly one code path. Called once from the daemon's startup
    /// task. A missing GitHub connection is not a failure - the background
    /// poller retries the moment a connection lands - so `NotConnected` only
    /// logs an info line; any other per-domain error is logged and never
    /// aborts startup. When env-origin domains exist while collaboration is
    /// off, one warning tells the operator to turn it on.
    pub async fn bootstrap_env_origins(&self) {
        let targets: Vec<(String, DomainEntry)> = self
            .overlay
            .env_domains()
            .filter(|(_, env)| env.entry.origin.is_some())
            .map(|(name, env)| (name.clone(), env.entry.clone()))
            .collect();
        if targets.is_empty() {
            return;
        }
        if !self.config.read().unwrap().github_enabled() {
            tracing::warn!(
                "env-defined team domains are configured but GitHub collaboration is off; set CRYSTALLINE_GITHUB_ENABLED=true to let them bootstrap"
            );
            return;
        }

        for (name, entry) in targets {
            let Ok(state_dir) = self.origin_state_dir(&name) else {
                continue;
            };
            // Already bootstrapped in an earlier run: nothing to do here, the
            // poller keeps it up to date from now on.
            let has_state = crystalline_remote::state::OriginState::load(&state_dir)
                .ok()
                .flatten()
                .is_some();
            if has_state {
                continue;
            }
            if !self.writes_domain_here(&name, false).await {
                tracing::debug!(
                    "env-defined team domain '{name}' is hosted by another instance; it bootstraps there"
                );
                continue;
            }
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => {
                    tracing::info!(
                        "bootstrapped env-defined team domain '{name}' ({} engram(s) at {})",
                        v["engrams"].as_u64().unwrap_or(0),
                        v["base_commit"].as_str().unwrap_or("")
                    );
                }
                Err(EngineError::Remote(RemoteError::NotConnected)) => {
                    tracing::info!(
                        "env-defined team domain '{name}' is waiting for a GitHub connection; the poller retries automatically"
                    );
                }
                Err(e) => {
                    tracing::warn!("could not bootstrap env-defined team domain '{name}': {e}");
                }
            }
        }
    }

    /// Reports where one origin-connected domain (or every one, when
    /// `domain` is `None`) stands relative to its origin, plus this
    /// machine's GitHub connection. Never hard-fails just because the
    /// machine is offline or has no saved connection: each domain's `behind`
    /// is `None` and the connection block reports `connected: false` rather
    /// than erroring. One domain's genuine failure (corrupt state, a missing
    /// filesystem root) never aborts the others: it is collected into the
    /// `errors` array instead, mirroring `origin_update`. Allowed on a
    /// read-only instance (a pure read).
    ///
    /// `detail` names the unshared work instead of only counting it: each
    /// domain entry then carries a `detail` block grouping the changed paths
    /// by kind (see `origin::local_change_detail`). It is opt-in because it
    /// costs a second walk of every domain's working tree, so `local_changes`
    /// stays the bare count for every caller that only wants to know whether
    /// there is anything to share.
    ///
    /// `diff` goes one step further and puts both sides of every unshared file
    /// in the detail block, under `diff`: the team's copy and this machine's,
    /// so a caller can say what changed before sharing or discarding it. It
    /// needs a domain, because reading every side of every domain at once is a
    /// walk nobody asked for, and it implies `detail`, since a diff with no
    /// file list beside it would be half an answer.
    pub async fn origin_status(
        &self,
        domain: Option<&str>,
        detail: bool,
        diff: bool,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if diff && domain.is_none() {
            return Err(EngineError::Invalid("diff needs a domain".to_string()));
        }
        let detail = detail || diff;
        let hidden = self.hidden_for(scope).await?;
        let targets = self.origin_targets(domain, &hidden)?;
        let mut connection = self.origin_status_connection().await?;
        if !matches!(
            scope,
            crate::scope::Scope::Unrestricted | crate::scope::Scope::User { admin: true, .. }
        ) {
            leave_out_for_non_admin(&mut connection);
        }

        let actor = crate::scope::overlay_actor(scope);
        // Who is asking, as a share would act: whose changes a `diff`
        // answers, and whose recorded refusal the status shows.
        let share_actor = match scope {
            crate::scope::Scope::Unrestricted => ShareActor::Owner,
            crate::scope::Scope::User { account, .. } => ShareActor::Account(account.clone()),
            crate::scope::Scope::Anonymous | crate::scope::Scope::Agent { .. } => {
                ShareActor::HttpAgent
            }
        };
        // A refusal is recorded per login, so the status reads it under the
        // login this caller's shares go out on. Resolved once, outside every
        // domain's lock; unresolvable reads as "shown none".
        let viewer = self.share_login_for(&share_actor);
        let diff_actor = diff.then_some(share_actor);
        let mut domains = Vec::new();
        let mut errors = Vec::new();
        for (name, entry) in targets {
            // Read here rather than inside the per-domain body, and that is not
            // tidiness: the body runs under this domain's origin lock, and
            // taking the store lock inside it would invent a lock pair that
            // exists nowhere else in the engine. The counts are a read of rows
            // nobody else in this call touches, so taking them first costs
            // nothing and orders nothing.
            //
            // Only for a domain that reviews changes, and only because the body
            // reports these keys for no other kind: a domain taking changes
            // directly would pay a store lock and two queries per status call
            // for an answer nothing reads, and the instance-wide `/sync`
            // overview asks this of every team domain at once. A fold that
            // stopped part way counts too: its drafts can still be settled,
            // so their authors are told which ones stand.
            let (view, converged) = if self.drafts_stay_open(&name) {
                // Whoever owns the domain sees who else is drafting in it. One
                // comparison covers the whole rule: an instance admin owns
                // every domain, a private domain's owner owns theirs, and
                // nobody else ever reaches `Own`.
                //
                // A membership that cannot be read is answered "no" rather than
                // propagated, and both halves of that are deliberate. A
                // permission question with no answer is not a yes, and this
                // function is documented never to fail a whole report over one
                // domain - a `?` here would abort every other domain's status
                // over one unreadable acl row.
                let everyone = match self.domain_right(scope, &name).await {
                    Ok(right) => right >= crate::scope::DomainRight::Own,
                    Err(e) => {
                        tracing::warn!(
                            domain = %name,
                            error = format!("{e:#}"),
                            "who holds domain '{name}' could not be read, so its status says \
                             nothing about who else is drafting there"
                        );
                        false
                    }
                };
                (
                    crate::review::DraftView::new(
                        self.overlay_counts_by_actor(&name).await,
                        actor.clone(),
                        everyone,
                    ),
                    // Read here for the reason the counts are: outside the
                    // per-domain lock, from what stands, and it fails no report.
                    self.converged_json(&name, actor.as_deref(), everyone).await,
                )
            } else {
                // Never read: the per-domain body reports the draft keys only
                // for a reviewing domain, which this is not.
                (
                    crate::review::DraftView::new(None, actor.clone(), false),
                    None,
                )
            };
            // Read out here for the reason the counts above are, and it is the
            // same reason: the per-domain body runs under that domain's origin
            // lock, and a reviewing domain's change list takes the store lock,
            // which would be exactly the lock pair this loop exists to keep
            // out of the body. The sides are read under no lock at all, which
            // is the promise `team_local_changes` already makes of every
            // offline read on this path.
            let diff_block = match diff_actor.as_ref() {
                Some(who) => match self.local_change_sides(&name, who).await {
                    Ok(sides) => Some(sides),
                    Err(e) => {
                        errors.push(json!({ "domain": name, "error": row_error(scope, &e) }));
                        continue;
                    }
                },
                None => None,
            };
            match self
                .origin_status_one(
                    &name,
                    &entry,
                    detail,
                    diff_block.as_ref(),
                    &view,
                    converged.as_ref(),
                    viewer.as_ref().map(Option::as_deref),
                )
                .await
            {
                Ok(v) => domains.push(v),
                Err(e) => errors.push(json!({ "domain": name, "error": row_error(scope, &e) })),
            }
        }
        Ok(json!({ "connection": connection, "domains": domains, "errors": errors }))
    }

    /// Reports one domain's status, under its origin lock. The per-domain
    /// body behind `origin_status`'s aggregate loop.
    ///
    /// A live probe is best-effort in two layers: no connection, or a
    /// provider that fails to build, degrades straight to `probe: None`
    /// (unchanged from before). When a provider was resolved but the probe
    /// call itself fails for a transport reason - offline, rate limited, an
    /// expired connection, see [`origin::is_probe_transport_error`] - the
    /// same domain is retried once with no probe at all, so the
    /// offline-capable report still comes back; the probe's own error
    /// message rides along verbatim as `probe_error` instead of aborting
    /// the domain. Any other failure (corrupt local state, and so on) is a
    /// genuine per-domain error, propagated to the caller's `errors` array.
    ///
    /// One thing this read is not allowed to do is settle an owed stack link.
    /// [`ops::status`] can pay that debt off with `create_stack`/`extend_stack`,
    /// which are forge WRITES, and the credential it would spend is the probe's
    /// own: the instance one, since a status carries no actor. In instance mode
    /// that is the credential every write goes out on anyway and the settlement
    /// runs exactly as it always has; in personal mode it would be the one
    /// instance-credential write the wave promises never happens, so permission
    /// is withheld and the debt stays recorded until the next share, amend or
    /// withdrawal pays it off on the acting identity's own credential.
    ///
    /// `detail` is threaded through both arms on purpose. The retry arm is the
    /// offline one, and offline is exactly when a caller cannot look the change
    /// list up anywhere else, so a status that degrades to local state still
    /// names what is unshared rather than dropping the one answer it can still
    /// give from the working tree alone.
    #[allow(clippy::too_many_arguments)]
    async fn origin_status_one(
        &self,
        name: &str,
        entry: &DomainEntry,
        detail: bool,
        diff: Option<&Value>,
        drafts: &crate::review::DraftView,
        converged: Option<&Value>,
        viewer: Option<Option<&str>>,
    ) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;
        // The mode in effect, as share changes use it: the count and the
        // detail below agree with the change list while a switch is pending.
        let reflection = self.reflection_mode(name).await?;
        // A probe is best-effort: no connection, or a provider that fails to
        // build, must never turn a status call into a hard failure.
        let probe = self.resolve_origin_provider().ok();
        let stacks = self.config.read().unwrap().github_stacks();
        let settle_owed_link =
            stacks && self.share_identity_loaded().mode == ShareIdentityMode::Instance;
        let change_detail = || {
            let mut block = detail
                .then(|| origin::local_change_detail(&root, &state_dir, reflection))
                .flatten();
            // Both sides ride inside the same block the paths do, so a caller
            // that asked for them reads one thing rather than two. In a
            // reviewing domain the block's own buckets name the working tree's
            // out-of-band files and this is what names the acting actor's
            // drafts, which is the only list a discard there can act on.
            if let (Some(block), Some(sides)) = (block.as_mut(), diff) {
                block["diff"] = sides.clone();
            }
            block
        };
        // In review mode every legitimate change joins its author's draft, so
        // anything the working tree holds that the origin does not got there
        // some other way: an editor, a script, a restored backup. It is
        // reported rather than blocked - the folder belongs to whoever holds
        // the machine - and the key is absent on a domain that takes changes
        // directly, where a local change is ordinary unshared work and
        // `local_changes` already says so.
        // Emitted for every reviewing domain, empty when nothing is known to be
        // unshared - the opposite of the rule `detail` follows beside it, and
        // deliberately. `detail` is absent when the walk never happened because
        // "nothing unshared" and "this could not be told" must not render the
        // same; here the key's presence is what says the domain reviews at all,
        // so making it absent for a domain that has never been pulled would say
        // "this domain takes changes directly", which is a different and wrong
        // thing.
        let out_of_band = entry.is_overlay().then(|| {
            origin::unshared_work(&root, &state_dir)
                .map(|work| work.paths)
                .unwrap_or_default()
        });
        let sharing = crystalline_core::sharing_at(&root);
        let app = self.origin_status_app(&spec.repo).await;
        let with_out_of_band = |mut value: Value| {
            if let Some(object) = value.as_object_mut() {
                // Read off the folder like the change detail is, so every
                // status surface says which kind of domain it is looking at.
                object.insert("sharing".to_string(), json!(sharing.as_str()));
                // Only with an App configured, so a status without one reads
                // exactly as it always has.
                if let Some(app) = &app {
                    object.insert("app".to_string(), app.clone());
                }
            }
            if let Some(paths) = &out_of_band
                && let Some(object) = value.as_object_mut()
            {
                object.insert("out_of_band".to_string(), json!(paths));
                // Under the same condition and for the same reason the block
                // above gives: on a reviewing domain every caller is told what
                // they are holding, and whoever owns the domain is told who
                // else is holding anything. On a domain that takes changes
                // directly neither key appears at all - `my_drafts: 0` there
                // would say "you are holding nothing here", which reads as
                // "you could be", and nobody can draft in a domain that is not
                // reviewing.
                object.insert("my_drafts".to_string(), drafts.mine());
                if let Some(everyone) = drafts.everyone() {
                    object.insert("drafts".to_string(), everyone);
                }
            }
            // What the last pull's convergence did here, absent until a
            // pull has converged something or flagged a conflict for this
            // domain - read off the durable record beside the journal, so
            // a restart does not turn "nothing has looked yet" into
            // "nothing converged". Absent rather than zeroed, for the
            // reason `behind` is null when nothing probed it: the two are
            // different answers. Read for a reviewing domain and for one
            // whose fold stopped part way, whose drafts still settle.
            if let Some(converged) = converged
                && let Some(object) = value.as_object_mut()
            {
                object.insert("converged".to_string(), converged.clone());
            }
            value
        };
        match ops::status(
            &spec,
            &root,
            &state_dir,
            probe.as_deref(),
            settle_owed_link,
            reflection,
        )
        .await
        {
            Ok(report) => Ok(with_out_of_band(origin::status_report_json(
                name,
                &report,
                None,
                change_detail(),
                origin::viewer_refusal(&report, viewer, sharing),
            ))),
            // A repository the App is not installed on degrades the same way:
            // the report still comes back from local state, with the refusal
            // (and its install url) as the probe error and in `app`.
            Err(e)
                if probe.is_some()
                    && (origin::is_probe_transport_error(&e)
                        || matches!(e, RemoteError::AppNotInstalled { .. })) =>
            {
                // AuthExpired is one of the transport errors this arm catches
                // (see `origin::is_probe_transport_error`), so a probe that
                // failed because the token was revoked drops the cached
                // credential here too; the retry below runs probe-free, so
                // status still comes back offline.
                self.drop_github_credential_on_auth(&e);
                let report =
                    ops::status(&spec, &root, &state_dir, None, settle_owed_link, reflection)
                        .await?;
                Ok(with_out_of_band(origin::status_report_json(
                    name,
                    &report,
                    Some(e.to_string()),
                    change_detail(),
                    origin::viewer_refusal(&report, viewer, sharing),
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The `app` block of one domain's status: `{ installed, install_url }`
    /// when a GitHub App is configured, `None` otherwise. `installed` is
    /// `null` when GitHub could not be asked (offline, rate limited), and
    /// `install_url` is `null` when the App's slug could not be read. Never
    /// an error: a status survives an App that cannot be reached.
    async fn origin_status_app(&self, repo: &str) -> Option<Value> {
        let github_app::AppSlot::Ready(router) = self.github_app_slot() else {
            return None;
        };
        let app = router.app();
        // Asked afresh rather than read from the cache: an uninstall since the
        // last call is exactly what this block exists to show.
        app.evict_repo(repo);
        let (installed, install_url) = match app.installation_for(repo).await {
            Ok(_) => (json!(true), app.install_url().await.ok()),
            Err(RemoteError::AppNotInstalled { install_url, .. }) => {
                (json!(false), Some(install_url))
            }
            Err(_) => (Value::Null, app.install_url().await.ok()),
        };
        Some(json!({ "installed": installed, "install_url": install_url }))
    }

    /// Runs one scheduling pass of the background origin poller: checks
    /// whether collaboration is enabled, connected and not paused for a
    /// shared rate limit, then brings every due origin-connected domain up
    /// to date via `Engine::origin_update_one`, the same per-domain pull
    /// an on-demand `origin_update` runs, under the same per-domain lock, so
    /// a poll tick and a concurrent on-demand update on the same domain
    /// never interleave. This method never talks to GitHub itself; it only
    /// decides which domains are due and delegates the actual pull, so
    /// polling and on-demand updating stay exactly one code path.
    ///
    /// `now` drives every due/not-due decision and `wall_now` is its
    /// wall-clock mirror, recorded alongside every reschedule so
    /// `status_report`'s offline `origins` block can show `next_due` without
    /// ever touching an `Instant` (which carries no epoch and cannot be
    /// serialized). Passing both in, rather than reading `Instant::now()`
    /// and `Utc::now()` here, is what lets a test drive several ticks
    /// deterministically with no real waiting.
    ///
    /// A tick does nothing when collaboration is off (so enabling it later
    /// starts polling on the very next tick, no restart needed), when the
    /// shared rate-limit pause has not yet elapsed or when no GitHub token
    /// is on file (so a `connect` lands and the next tick picks it up
    /// automatically; a debug line notes this at most once an hour). A
    /// domain hitting `RemoteError::RateLimited` pauses every domain until
    /// the reported reset (defaulting an hour out when GitHub reports none)
    /// and ends the tick immediately, since GitHub rate limits are
    /// per-token, not per-repository. Any other per-domain failure (offline,
    /// a revoked token, a corrupt state directory) is recorded quietly and
    /// never stops the tick from moving on to the next due domain. A tick
    /// whose pulls landed anything ends with [`Engine::adopt_domain_names`],
    /// since a pulled MANIFEST may declare a new name.
    pub async fn origin_poll_tick(&self, now: Instant, wall_now: DateTime<Utc>) {
        if !self.config.read().unwrap().github_enabled() {
            return;
        }
        if let Some(until) = self.origin_poller.rate_limited_until() {
            if wall_now < until {
                return;
            }
            self.origin_poller.set_rate_limited_until(None);
        }
        // An App is a connection of its own: an instance that runs one needs
        // no instance token to pull. Asked only when no token is on file, so
        // an instance without an App reads exactly what it read before.
        if !self.origin_connection_offline().0
            && !matches!(self.github_app_slot(), github_app::AppSlot::Ready(_))
        {
            if self.origin_poller.should_log_no_token(now) {
                tracing::debug!(
                    "origin poll: no GitHub connection yet; waiting for connect to resume polling"
                );
            }
            return;
        }
        // The poller is the machine itself rather than a caller, so no privacy
        // is subtracted: it polls every origin this daemon hosts, except a
        // local copy a connected server hides, whose files stay as they are
        // until it comes back on disconnect (spec A3).
        let Ok(targets) = self.origin_targets(None, &self.shadowed_domains()) else {
            return;
        };
        let github_poll_secs = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.poll_secs);

        // Whether any pull of this tick landed something, which is when a
        // MANIFEST may declare a new name.
        let mut moved = false;
        // Which rename questions were already respelled here before this
        // tick's pulls (see `origin_update`), asked before the first pull.
        let mut clean_before: Option<HashSet<String>> = None;
        for (name, entry) in targets {
            if !self.origin_poller.is_due(&name, now) {
                continue;
            }
            let domain_poll_secs = entry.origin.as_ref().and_then(|o| o.poll_secs);
            let interval_secs = poller::effective_interval_secs(domain_poll_secs, github_poll_secs);
            let tick = self.origin_poller.next_tick();
            let jitter = poller::jittered_interval(interval_secs, &name, tick);
            let jitter_chrono =
                Duration::from_std(jitter).unwrap_or(Duration::seconds(interval_secs as i64));
            self.origin_poller
                .schedule(&name, now + jitter, wall_now + jitter_chrono);

            // A domain another live instance hosts is that instance's to
            // pull: skipped quietly, never claimed from a live host.
            if !self.writes_domain_here(&name, false).await {
                tracing::debug!("origin poll: '{name}' is hosted by another instance; skipped");
                continue;
            }
            if clean_before.is_none() {
                clean_before = Some(self.rename_questions_already_clean().await);
            }
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => {
                    let up_to_date = v["up_to_date"].as_bool().unwrap_or(false);
                    moved |= !up_to_date;
                    let applied = v["applied"].as_array().map(Vec::len).unwrap_or(0);
                    let conflict_paths: Vec<&str> = v["conflicts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["path"].as_str())
                        .collect();
                    // A share proposal can transition (merged, declined) with
                    // no file in this domain changing at all, so it needs its
                    // own info line even when the pull otherwise reports
                    // `up_to_date`: `PullReport::proposals` (see
                    // `crystalline_remote::ops::settle_up_to_date`) is
                    // refreshed on every pull regardless of whether the
                    // branch itself moved.
                    let proposal_lines: Vec<String> = v["proposals"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| {
                            let number = p["number"].as_u64().unwrap_or(0);
                            let status = p["status"].as_str().unwrap_or("?");
                            format!("#{number} {status}")
                        })
                        .collect();
                    if v["bootstrapped"].as_bool().unwrap_or(false) {
                        tracing::info!(
                            "origin poll: bootstrapped '{name}' ({} engram(s))",
                            v["engrams"].as_u64().unwrap_or(0)
                        );
                    } else if !conflict_paths.is_empty() {
                        tracing::info!(
                            "origin poll: '{name}' has new conflict(s): {}",
                            conflict_paths.join(", ")
                        );
                    } else if !proposal_lines.is_empty() {
                        tracing::info!(
                            "origin poll: '{name}' proposal update: {}",
                            proposal_lines.join(", ")
                        );
                    } else if !up_to_date {
                        tracing::info!("origin poll: '{name}' applied {applied} file(s)");
                    } else {
                        tracing::debug!("origin poll: '{name}' is up to date");
                    }
                    let outcome = if up_to_date {
                        poller::DomainPollOutcome::UpToDate
                    } else {
                        poller::DomainPollOutcome::Applied {
                            applied,
                            conflicts: conflict_paths.len(),
                        }
                    };
                    self.origin_poller.record_result(&name, outcome);
                }
                Err(EngineError::Remote(RemoteError::RateLimited { reset })) => {
                    let until = reset.unwrap_or_else(|| wall_now + Duration::hours(1));
                    tracing::warn!(
                        "origin poll: GitHub is rate limiting this machine; pausing every domain until {until}"
                    );
                    self.origin_poller.set_rate_limited_until(Some(until));
                    break;
                }
                Err(e) => {
                    tracing::debug!("origin poll: '{name}' failed: {e}");
                    self.origin_poller
                        .record_result(&name, poller::DomainPollOutcome::Error(e.to_string()));
                }
            }
        }
        // After the loop, when every pull has let go of its origin lock.
        if moved {
            self.adopt_domain_names_after("the origin poll").await;
            self.settle_rename_questions_after_pull(&clean_before.unwrap_or_default())
                .await;
        }
    }

    /// This machine's GitHub connection for `status_report`'s offline
    /// `origins` block: `(connected, token_store)`. Unlike
    /// `origin_connection_json` (used by the live `origin_status` operation,
    /// which reflects an injected test provider's own identity as always
    /// connected), this never special-cases an injected provider: it is a
    /// plain token-store lookup, exactly the same check the poller itself
    /// makes before spending a tick on any domain, so the two never
    /// disagree about whether this machine is connected.
    fn origin_connection_offline(&self) -> (bool, Option<&'static str>) {
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        match self.github_credential(host.as_deref()) {
            Ok((store, Some(_))) => (true, Some(store.kind())),
            _ => (false, None),
        }
    }

    /// Builds `status_report`'s `origins` block entirely offline: this
    /// machine's GitHub connection, the poller's shared rate-limit pause
    /// and, per origin-connected domain, its repo, branch, proposal and
    /// conflict counts and local change count from a probe-free
    /// `ops::status` call (the same state-only read `origin_status` itself
    /// falls back to when a live probe fails), plus the poller's own
    /// schedule and last result for that domain. Every read here is local:
    /// the token store and each domain's saved origin state, never a GitHub
    /// call, so `status` never blocks on the network even when
    /// collaboration is on.
    pub(super) async fn origins_status_block(&self) -> Value {
        let (connected, token_store) = self.origin_connection_offline();
        let rate_limit_wait_until = self.origin_poller.rate_limited_until();
        // `status`'s own block, which the CLI and the control socket read: the
        // machine owner, so nothing is subtracted.
        let targets = self
            .origin_targets(None, &HashSet::new())
            .unwrap_or_default();

        let mut domains = Vec::new();
        for (name, entry) in targets {
            let Ok((spec, root, state_dir)) = self.origin_spec_for(&name, &entry) else {
                continue;
            };
            let reflection = self.reflection_mode(&name).await.unwrap_or_default();
            // No provider, so nothing here could settle an owed stack link
            // anyway; withholding the permission says so at the call rather
            // than leaving it to be inferred from the `None` beside it.
            let Ok(report) = ops::status(&spec, &root, &state_dir, None, false, reflection).await
            else {
                continue;
            };
            let next_due = self.origin_poller.next_due_at(&name);
            let last_result = self.origin_poller.last_result(&name);
            let mut entry =
                origin::origin_poll_status_json(&name, &report, next_due, last_result.as_ref());
            entry["sharing"] = json!(crystalline_core::sharing_at(&root).as_str());
            domains.push(entry);
        }

        json!({
            "enabled": true,
            "connected": connected,
            "token_store": token_store,
            "rate_limit_wait_until": rate_limit_wait_until,
            "domains": domains,
        })
    }

    /// Whose drafts a share of `domain` is of, or `None` when the domain takes
    /// changes directly and a share is the folder walk it always was.
    ///
    /// Asked FIRST by both share verbs, ahead of the credential and ahead of
    /// every forge call: an agent with no identity holds no draft, so there is
    /// nothing for a share to be of whatever this instance's GitHub
    /// configuration says, and the refusal that teaches the way in is the one a
    /// write already gets. Resolving a credential before saying so would answer
    /// a question about the forge to a caller whose problem is that nobody knows
    /// whose work this is.
    fn overlay_share_identity(&self, domain: &str, actor: &ShareActor) -> Result<Option<String>> {
        if !self.reviews_changes(domain) {
            return Ok(None);
        }
        share_staging::overlay_share_actor(actor).map(Some)
    }

    /// The tree a share or a preview of `domain` runs against, or `None` when
    /// `drafting` is `None` and the folder on disk is the answer it always was.
    ///
    /// The single seam review mode adds to sharing, so the share and the preview
    /// Fluid's share dialog shows cannot describe two different things.
    /// Two steps, in this order:
    ///
    /// 1. a pull of the REAL folder, which is the pull `ops::propose` would run
    ///    for itself. It has to happen against the folder rather than the staged
    ///    tree: a pull advances the base snapshot as it applies upstream work,
    ///    so a pull into staging would leave the team's folder behind its own
    ///    base, and no later pull would bring it back. Run here, it lands where
    ///    it belongs;
    /// 2. the staged tree itself ([`Engine::stage_overlay_share`]), and with it
    ///    the commit the folder now stands at. The caller hands that to
    ///    [`crate::share_staging::PinnedHead`], which is what turns "the inline
    ///    pull has nothing left to do" from likely into true.
    ///
    /// `origin` is what [`Engine::origin_spec_for_domain`] resolved, borrowed
    /// whole: the spec, the domain's folder and its origin state directory.
    ///
    /// `cleanup` is what the pull does with a queued share branch. A share and
    /// a preview both pass `Defer`: the preview makes no provider write, and
    /// the share's own pull in `ops::propose` runs the cleanup, once.
    ///
    /// `pass` is whose drafts the convergence pass may write (see
    /// [`SharePass`]). The answer carries the acting person's conflicts, read
    /// after the pass and before the tree is staged.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn overlay_share_tree(
        &self,
        domain: &str,
        drafting: Option<&str>,
        provider: &dyn Provider,
        origin: (&OriginSpec, &Path, &Path),
        acting: Option<&str>,
        cleanup: ops::BranchCleanup,
        pass: SharePass,
    ) -> Result<Option<PreparedShare>> {
        let Some(who) = drafting else {
            return Ok(None);
        };
        let (spec, root, state_dir) = origin;
        let report = ops::pull_with(provider, spec, root, state_dir, cleanup)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))
            .map_err(|e| enrich_write_error(e, acting, &spec.repo))?;
        // This pull advances the base like any other, so the same convergence
        // runs on it - and it runs BEFORE the tree is staged, so a draft the
        // team has already merged is not proposed straight back at them. It
        // runs on every call, whether or not this pull applied anything: an
        // earlier pull or a direct commit whose pass failed leaves drafts a
        // pass can still merge. Its failure stays a warning, because the gate
        // is the conflict list each caller asks next, which reads what stands
        // and so refuses a stale draft whether or not a pass reached it.
        //
        // The sync in front of it is what makes the two call sites one rule
        // rather than two. A rename finishes through `write_overlay_entry`,
        // whose address check reads index rows; against rows that still
        // describe the pre-pull folder the old base row would still hold the
        // draft's address and the destination write would be refused, so the
        // same rename would come out a move here and a divergence there. The
        // scan is incremental, and the share's own tail syncs again after the
        // proposal, where a second pass over unchanged files costs a walk.
        if !report.up_to_date
            && let Err(e) = self.sync(Some(domain)).await
        {
            tracing::warn!("indexing what the pull before sharing '{domain}' applied: {e}");
        }
        // The pass belongs to the pull: one that applied something runs it for
        // every person, whatever the caller may write, since its renames and
        // divergences are read from this pull's own paths once and never again.
        // Only an up-to-date pull narrows the pass to what the caller asked.
        let only = match pass {
            _ if !report.up_to_date => Some(None),
            SharePass::Everyone => Some(None),
            SharePass::Own => Some(Some(who)),
            SharePass::Nobody => None,
        };
        if let Some(only) = only
            && let Err(e) = self
                .converge_overlays_of(domain, &report.applied, only)
                .await
        {
            tracing::warn!("converging the drafts in '{domain}' before sharing failed: {e}");
        }
        // The gate's list, read before the tree is staged: a step that settles a
        // conflict between the two can then only make a refusal stricter, never
        // let a tree staged from a stale row travel. A list that cannot be read
        // refuses, and is never read as empty.
        let conflicts = self.draft_conflicts(domain, who).await?;
        Ok(Some(
            self.stage_overlay_share(domain, who, state_dir, conflicts)
                .await?,
        ))
    }

    /// Reads what one actor's share is made of - the base snapshot the pull just
    /// settled and that actor's own index rows - and hands both to
    /// [`crate::share_staging::build`].
    ///
    /// Thin on purpose: the filesystem work and its lifetime live in
    /// [`crate::share_staging`], and what belongs here is which two things are
    /// read and with which lookup. **The rows, never the journal beside them**:
    /// the journal is the durable mirror a rebuilt index is restored from, and a
    /// mirror that had fallen behind would quietly change what a share carries.
    /// And **the read-only id lookup**, never an upserting one: a share of a
    /// domain this index has never been told about holds no drafts, and asking
    /// must not register one.
    async fn stage_overlay_share(
        &self,
        domain: &str,
        actor: &str,
        state_dir: &Path,
        conflicts: Vec<DraftConflict>,
    ) -> Result<PreparedShare> {
        let state = crystalline_remote::state::OriginState::load(state_dir)?.ok_or_else(|| {
            EngineError::Conflict(format!(
                "domain '{domain}' has no origin state; add the domain from its origin first"
            ))
        })?;
        let view = DomainView::for_actor(self, domain, &HashSet::new(), actor)?;
        Ok(PreparedShare {
            staging: view.materialise(state_dir, &state.files).await?,
            pinned: state.base_commit,
            conflicts,
        })
    }

    /// [`Engine::origin_share_with`] without `as_proposal`: the share goes
    /// the way the domain and this machine's recorded refusal decide.
    pub async fn origin_share(
        &self,
        domain: &str,
        title: Option<&str>,
        description: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        actor: ShareActor,
    ) -> Result<Value> {
        self.origin_share_with(domain, title, description, proposal, files, None, actor)
            .await
    }

    /// Proposes one domain's local changes as a pull request against its
    /// origin, under its origin lock.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (a share
    /// publishes content, exactly what read-only mode protects). When
    /// `ops::propose` refuses because conflicts are still pending, this
    /// degrades that refusal into a `conflicts_pending` outcome carrying the
    /// actual conflict paths (reloaded from the domain's now-current state,
    /// durable on disk since the inline pull inside `propose` already
    /// persisted them) rather than the bare count `RemoteError` alone
    /// carries, so a caller never needs to make a second round trip to learn
    /// what needs resolving. The share itself never touches local files, but
    /// the pull it opens with does, so it ends with the same sync and embed
    /// tail `origin_update_one` runs (see
    /// `Engine::index_what_the_share_pull_applied`).
    ///
    /// `proposal` names an open layer to amend instead of letting the share
    /// pick its own target; `None` is the ordinary call.
    ///
    /// `files` narrows what the share carries to those domain-relative paths,
    /// the generated listings of their folders riding along; `None` is the
    /// whole unshared delta. A path that is not among the domain's unshared
    /// changes refuses the share by name (see
    /// `crystalline_remote::ops::ShareOptions::files`).
    ///
    /// `actor` is who the share runs as, which decides the credential the
    /// forge writes go out on (see `Engine::resolve_share_provider`); it is
    /// inert while the share identity mode is `instance`.
    ///
    /// The login that credential was connected as is recorded on the proposal
    /// this share creates or rewrites, in both modes
    /// (`crystalline_remote::state::Proposal::author_login`), so a chain whose
    /// layers belong to different people can say so.
    ///
    /// `as_proposal` is how a share on a `sharing: direct` domain goes:
    /// `Some(true)` opens a proposal, `Some(false)` tries the direct commit
    /// only, `None` lets the refusal recorded for the acting login decide (see
    /// `crystalline_remote::ops::ShareOptions::as_proposal`).
    #[allow(clippy::too_many_arguments)]
    pub async fn origin_share_with(
        &self,
        domain: &str,
        title: Option<&str>,
        description: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        as_proposal: Option<bool>,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // The policy, read off the REAL folder and never off the tree the
        // share detects in: in review mode that tree holds the actor's own
        // draft of the MANIFEST, and a draft must not switch the review step
        // off for its own author.
        let sharing = crystalline_core::sharing_at(&root);
        let drafting = self.overlay_share_identity(domain, &actor)?;
        if let (Some(who), true) = (drafting.as_deref(), stacks_allowed) {
            self.refuse_open_proposal_while_reviewing(domain, &state_dir, who)?;
        }
        let (provider, login) = self.resolve_share_provider(&actor, &spec.repo).await?;
        let acting = self.personal_write_login(login.as_deref());
        // The team marker, for a share of the folder: written into the folder
        // before anything is detected (see `Engine::plan_team_marker`). A
        // failure never fails the share; the marker waits for the next one.
        let mut marker: Option<TeamMarker> = None;
        if drafting.is_none() {
            match self
                .plan_team_marker(domain, &root, &state_dir, files)
                .await
            {
                Ok(Some(plan)) => match self.write_team_marker(domain, &root, &plan.marked).await {
                    Ok(true) => {
                        marker = Some(TeamMarker {
                            in_folder: true,
                            ..plan
                        })
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(
                        "writing the team marker of '{domain}' before a share failed: {e}"
                    ),
                },
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("planning the team marker of '{domain}' failed: {e}")
                }
            }
        }
        // In review mode the share is of the actor's own drafts, which are in
        // the index and on nobody's disk, so the tree `ops` detects against is
        // staged here. `None` is a domain that takes changes directly, whose
        // share is the folder walk it always was.
        let staging = self
            .overlay_share_tree(
                domain,
                drafting.as_deref(),
                provider.as_ref(),
                (&spec, &root, &state_dir),
                acting.as_deref(),
                ops::BranchCleanup::Defer,
                SharePass::Everyone,
            )
            .await?;
        // Somebody else's proposal is open: this share waits for it, read
        // after the pull above so one the forge merged or closed is gone.
        if let Some(who) = drafting.as_deref()
            && let Some((open, owner)) =
                self.another_members_open_proposal(domain, &state_dir, who)?
        {
            self.index_what_the_share_pull_applied(domain, "sharing")
                .await;
            return Ok(origin::another_members_proposal_json(
                &open,
                &review_proposal_is_anothers(owner.as_deref(), open.number),
            ));
        }
        // The share runs against the staged tree, and the provider it runs with
        // holds the inline pull to the commit that tree was staged over: a merge
        // landing in staging would be deleted with it while the base snapshot
        // advanced past it, and nothing would ever put it back.
        let pinned = staging.as_ref().map(|prepared| {
            share_staging::PinnedHead::new(provider.as_ref(), prepared.pinned.clone())
        });
        let share_provider: &dyn Provider = match &pinned {
            Some(pinned) => pinned,
            None => provider.as_ref(),
        };
        let detect_in = staging
            .as_ref()
            .map_or(root.as_path(), |prepared| prepared.staging.root());
        // In review mode the marker joins the staged tree of the actor's
        // drafts, so it goes through the review like any other line; the
        // folder moves only when the team merges it.
        if let Some(prepared) = staging.as_ref() {
            let tree = prepared.staging.root();
            match self.plan_team_marker(domain, tree, &state_dir, files).await {
                Ok(Some(plan)) => match write_file(&tree.join(MANIFEST_PATH), &plan.marked) {
                    Ok(()) => marker = Some(plan),
                    Err(e) => tracing::warn!(
                        "staging the team marker of '{domain}' for a share failed: {e}"
                    ),
                },
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("planning the team marker of '{domain}' failed: {e}")
                }
            }
        }
        let files = marker
            .as_ref()
            .and_then(|plan| plan.files.as_deref())
            .or(files);
        // The gate of a reviewing domain: a draft of hers that does not fit the
        // team's text, among what this share carries, refuses it. The list was
        // read after the pull and its pass and before the tree was staged (see
        // `Engine::overlay_share_tree`); only the files it is matched against
        // are known here, after the marker step. In review mode the marker is
        // only staged, so there is nothing to take back.
        if let Some(prepared) = staging.as_ref() {
            let blocking = draft_conflicts::conflicts_block(&prepared.conflicts, files);
            if !blocking.is_empty() {
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
                return Ok(origin::draft_conflicts_pending_json(&blocking));
            }
        }
        // Read before the share, whose convergence may end these rows.
        let unshared_spelling = match drafting.as_deref() {
            Some(who) => self.case_only_moves(domain, who, &state_dir).await?,
            None => Vec::new(),
        };
        match ops::propose(
            share_provider,
            &spec,
            detect_in,
            domain,
            &state_dir,
            ops::ShareOptions {
                as_proposal,
                title,
                description,
                note: marker.is_some().then_some(TEAM_MARKER_NOTE),
                proposal,
                stacks_allowed,
                // Who the proposal record names, in either identity mode: the
                // credential this share actually went out on. `None` only when
                // that credential carries no login (the environment token).
                author_login: login.as_deref(),
                files,
                sharing,
                // The mode in effect: a pending switch keeps sharing the
                // engrams still under `reflections/` until they move out.
                reflection: self.reflection_mode(domain).await?,
            },
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        {
            Ok(outcome) => {
                let mut receipt = origin::propose_outcome_json(&outcome);
                note_unshared_spelling(&mut receipt, &unshared_spelling);
                // Only an outcome that carried something to the team carried
                // the marker; a refusal or a diverged proposal shared nothing,
                // and the marker it wrote into the folder is taken back.
                let shared = matches!(
                    outcome,
                    ops::ProposeOutcome::Proposed(_)
                        | ops::ProposeOutcome::Updated(_)
                        | ops::ProposeOutcome::Committed(_)
                        | ops::ProposeOutcome::FellBack { .. }
                );
                if let Some(plan) = &marker {
                    if shared {
                        receipt["team_marker"] = team_marker_json();
                    } else {
                        self.take_back_team_marker(domain, &root, plan).await;
                    }
                }
                // A reviewing domain's direct commit: the folder is written
                // from the base copies the commit advanced, those paths are
                // indexed, and the convergence pass a merged proposal's pull
                // would run runs now - the next pull finds head == base and
                // would never run it.
                if let (Some(who), ops::ProposeOutcome::Committed(report)) =
                    (drafting.as_deref(), &outcome)
                {
                    let paths: Vec<String> = report
                        .added
                        .iter()
                        .chain(&report.updated)
                        .chain(&report.deleted)
                        .cloned()
                        .collect();
                    // Nothing here may turn a landed commit into an error:
                    // the branch already moved and no retry can take it back,
                    // so a local IO or store failure is warned about and the
                    // `committed` receipt stands. The reviewed folder then
                    // differs from its base copies, which `origin_status`
                    // counts as local changes and `discard_changes` restores.
                    // An update does not heal a failed fold on its own: after
                    // the failure the head equals the base, so its pull is up
                    // to date and runs no convergence. Her next share or
                    // preview does, since those run the pass on every call
                    // (a preview for her own drafts only); otherwise the
                    // drafts fold once a later upstream write touches those
                    // paths, or when the person discards them.
                    if let Err(e) = ops::materialise_base_paths(&root, &state_dir, &paths) {
                        tracing::warn!("writing the direct commit's files in '{domain}': {e}");
                    }
                    if let Err(e) = self.sync_paths(domain, paths.clone()).await {
                        tracing::warn!("indexing the direct commit's files in '{domain}': {e}");
                        // The commit stands; the answer says why its files
                        // are not indexed here (another instance hosts the
                        // domain, for a command on a shared database).
                        receipt["index_error"] = json!(e.to_string());
                    }
                    // Before the pass, which would otherwise read a draft whose
                    // start this commit deleted as the team deleting it.
                    if let Some(said) = self
                        .free_drafts_of_shared_deletions(domain, who, &report.deleted)
                        .await
                    {
                        receipt["free_error"] = json!(said);
                    }
                    match self.converge_pulled_overlays(domain, &paths).await {
                        Ok(folded) => receipt["drafts_folded"] = json!(folded.cleared),
                        Err(e) => {
                            tracing::warn!("folding the shared drafts in '{domain}': {e}");
                            // The one key that says the commit landed and the
                            // drafts behind it did not fold, in place of the
                            // count a fold that ran would have carried.
                            receipt["fold_error"] = json!(e.to_string());
                        }
                    }
                }
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
                // Whose the proposal is, in the sense review mode means it. The
                // forge record cannot say (see
                // `Engine::refuse_open_proposal_while_reviewing`), so it is
                // recorded here, beside the drafts it carried. So is what it
                // holds of each draft, which becomes the draft's start once the
                // team merges it.
                if let Some(who) = drafting.as_deref()
                    && let Some(number) = proposal_number_of(&receipt)
                {
                    self.record_proposal_actor(domain, number, Some(who));
                    // An update can land on a proposal the team merged after
                    // this share's pull: a forge keeps a merged branch, so the
                    // push goes through and the merge never held it. Asked once,
                    // after the push: a merge from here on holds what was pushed.
                    let merged_meanwhile = match &outcome {
                        ops::ProposeOutcome::Updated(_) => {
                            match provider.proposal_state(&spec, number).await {
                                Ok(crystalline_remote::provider::ProposalState::Open) => None,
                                Ok(state) => Some(
                                    state == crystalline_remote::provider::ProposalState::Merged,
                                ),
                                Err(e) => {
                                    tracing::warn!(
                                        "asking whether proposal #{number} of '{domain}' is still \
                                         open after its update failed, so what it holds is not \
                                         recorded: {e}"
                                    );
                                    Some(false)
                                }
                            }
                        }
                        _ => None,
                    };
                    self.record_what_the_proposal_holds(
                        domain,
                        who,
                        &state_dir,
                        number,
                        merged_meanwhile.is_none(),
                    )
                    .await;
                    if merged_meanwhile == Some(true) {
                        receipt["note"] = json!(merged_before_the_update(number));
                    }
                }
                Ok(receipt)
            }
            Err(RemoteError::ConflictsPending { count }) => {
                if let Some(plan) = &marker {
                    self.take_back_team_marker(domain, &root, plan).await;
                }
                // The conflicts refusal is the loudest case for syncing: the
                // pull ran, applied everything that merged cleanly and only
                // then refused, so this shape carries the most unindexed work
                // of any share outcome.
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
                let conflicts = crystalline_remote::state::OriginState::load(&state_dir)
                    .ok()
                    .flatten()
                    .map(|s| s.conflicts)
                    .unwrap_or_default();
                Ok(json!({
                    "outcome": "conflicts_pending",
                    "count": count,
                    "conflicts": conflicts,
                }))
            }
            Err(e) => {
                if let Some(plan) = &marker {
                    self.take_back_team_marker(domain, &root, plan).await;
                }
                Err(share_write_error(e, acting.as_deref(), &spec.repo).into())
            }
        }
    }

    /// After a share that opened or amended proposal `number`: each of
    /// `actor`'s rows the proposal carries records what the branch holds of it
    /// (`DraftStart::shared`), so that once the team merges it the draft starts
    /// from that text and a reviewer's change reads as theirs (see
    /// `Engine::settle_shared_texts`).
    ///
    /// A file the proposal changes maps to her draft at that path, or to her
    /// case move whose record starts there: a share does not carry a case-only
    /// change of spelling, so `plan.md` holds her `Plan.md`. A file it deletes
    /// maps to her tombstone at that path.
    ///
    /// **Only the text the branch holds is recorded.** Draft writers do not take
    /// the origin lock, so a row can change between staging and here; it is
    /// read again under its draft lock and recorded only when its digest is the
    /// one the proposal recorded, in either line ending, or keeps what an
    /// earlier share of the same proposal recorded when that text is still the
    /// digest. Any other row forgets what was recorded: a newer text taken as
    /// the start would read her newer words as no edit, and the team's text
    /// would replace them. A row with no record is an old draft, which nothing
    /// here makes into a known start.
    ///
    /// **Every record of anyone's that names this proposal and that this share
    /// did not carry forgets it.** The share wrote the whole branch: an amend
    /// proposes exactly what it carries, and a colleague's share can update the
    /// same proposal in place with only his drafts.
    ///
    /// Best effort, like the rest of the share's tail: the proposal is on the
    /// forge, so a failure is warned about, one row at a time, and the others
    /// go on. A record that still names the proposal after a failure is
    /// settled only where the merged proposal's own record lists its text
    /// (`draft_merge::settle_shared`), so it can refuse but loses nothing.
    ///
    /// With `holds` false the proposal holds nothing of anybody's any more
    /// (an update that landed after the team merged or closed it): every
    /// record that names it forgets it, and each row merges against the start
    /// it had.
    async fn record_what_the_proposal_holds(
        &self,
        domain: &str,
        actor: &str,
        state_dir: &Path,
        number: u64,
        holds: bool,
    ) {
        self.take_proposal_record_hold().await;
        let warn = |path: &str, e: &dyn std::fmt::Display| {
            tracing::warn!(
                domain,
                actor,
                path,
                "what proposal #{number} holds of this draft could not be recorded; it merges \
                 against where it started: {e}"
            );
        };
        let read = async {
            let Some(state) = crystalline_remote::state::OriginState::load(state_dir)? else {
                return Ok(None);
            };
            let Some(proposal) = state.proposals.into_iter().find(|p| p.number == number) else {
                return Ok(None);
            };
            let store = self.store.lock().await;
            let Some(domain_id) = store.domain_id(domain).await? else {
                return Ok(None);
            };
            let rows = store.overlay_entries(domain_id, actor).await?;
            let actors: Vec<String> = store
                .overlay_counts(domain_id)
                .await?
                .into_iter()
                .map(|(actor, _)| actor)
                .collect();
            Ok::<_, EngineError>(Some((proposal, domain_id, rows, actors)))
        };
        let (proposal, domain_id, rows, actors) = match read.await {
            Ok(Some(read)) => read,
            Ok(None) => return,
            Err(e) => return warn("", &e),
        };
        let journal_dir = match self.journal_state_dir() {
            Ok(dir) => dir,
            Err(e) => return warn("", &e),
        };
        let started_at = |path: &str| {
            crate::draft_start::load(&journal_dir, domain, actor, path)
                .ok()
                .flatten()
                .and_then(|record| record.from)
        };
        let fold = crystalline_core::fold_path_case;
        let mut carried_rows: HashSet<String> = HashSet::new();
        let carried_files = if holds {
            proposal.files.as_slice()
        } else {
            &[]
        };
        for file in carried_files {
            let deletes = file.change == crystalline_remote::state::ProposedChange::Deleted;
            let carried = rows
                .iter()
                .find(|row| row.tombstone == deletes && row.path == file.path)
                .or_else(|| {
                    rows.iter().find(|row| {
                        !deletes
                            && !row.tombstone
                            && row.path != file.path
                            && fold(&row.path) == fold(&file.path)
                            && started_at(&row.path).as_deref() == Some(file.path.as_str())
                    })
                });
            let Some(path) = carried.map(|row| row.path.clone()) else {
                continue;
            };
            carried_rows.insert(path.clone());
            if let Err(e) = self
                .record_one_shared_row(domain, domain_id, actor, &journal_dir, &path, file, number)
                .await
            {
                warn(&path, &e);
            }
        }
        self.record_shared_files(domain, actor, &journal_dir, carried_files, number)
            .await;
        for who in actors {
            let records = match crate::draft_start::all(&journal_dir, domain, &who) {
                Ok(records) => records,
                Err(e) => {
                    warn("", &e);
                    continue;
                }
            };
            for record in records {
                let named = record.shared.as_ref().is_some_and(|s| s.proposal == number);
                if !named || (who == actor && carried_rows.contains(&record.path)) {
                    continue;
                }
                let forgotten = async {
                    let lock = self.draft_lock(domain, &who, &record.path)?;
                    let _guard = lock.lock().await;
                    let now = crate::draft_start::load(&journal_dir, domain, &who, &record.path)
                        .map_err(|source| EngineError::Io {
                            path: format!("the draft start record of '{}'", record.path),
                            source,
                        })?;
                    if let Some(now) = now
                        && now.shared.as_ref().is_some_and(|s| s.proposal == number)
                    {
                        self.save_start(
                            domain,
                            &who,
                            &crate::draft_start::DraftStart {
                                shared: None,
                                ..now
                            },
                        )?;
                    }
                    Ok::<(), EngineError>(())
                };
                if let Err(e) = forgotten.await {
                    warn(&record.path, &e);
                }
            }
        }
    }

    /// After a share that opened or amended proposal `number`: each of
    /// `actor`'s draft files the proposal carries records what it holds of it
    /// (`DraftFileStart::shared`), as `record_one_shared_row` does for a row.
    /// Under the file's lock, and only when her bytes now are the ones the
    /// proposal recorded, or her deletion is one it makes. A file that keeps
    /// what an earlier share of the same proposal recorded keeps it. Every
    /// other record of anyone's that names this proposal forgets it.
    ///
    /// Best effort: a failure is warned about and the record keeps its start,
    /// which reads as a conflict after the merge rather than losing anything.
    async fn record_shared_files(
        &self,
        domain: &str,
        actor: &str,
        journal_dir: &Path,
        carried: &[crystalline_remote::state::ProposedFile],
        number: u64,
    ) {
        let files = self.overlay_domain_files(domain);
        for (who, read) in &files.per_actor {
            for entry in &read.entries {
                let carries = (who == actor)
                    .then(|| carried.iter().find(|file| file.path == entry.path))
                    .flatten();
                if let Err(e) = self
                    .record_one_shared_file(domain, who, journal_dir, &entry.path, carries, number)
                    .await
                {
                    tracing::warn!(
                        domain,
                        actor = who.as_str(),
                        path = entry.path.as_str(),
                        "what proposal #{number} holds of this draft file could not be \
                         recorded: {e}"
                    );
                }
            }
        }
    }

    /// [`Engine::record_shared_files`] for one file, under its write lock.
    async fn record_one_shared_file(
        &self,
        domain: &str,
        actor: &str,
        journal_dir: &Path,
        path: &str,
        carries: Option<&crystalline_remote::state::ProposedFile>,
        number: u64,
    ) -> Result<()> {
        let io = |source| EngineError::Io {
            path: format!("the draft start record of the file '{path}'"),
            source,
        };
        let abs = crate::overlay_files::file(journal_dir, domain, actor, path).map_err(io)?;
        let lock = self.write_lock(&abs);
        let _guard = lock.lock().await;
        let Some(record) =
            crate::draft_start::load_file(journal_dir, domain, actor, path).map_err(io)?
        else {
            return Ok(());
        };
        let names_this = record
            .shared
            .as_ref()
            .is_some_and(|shared| shared.proposal == number);
        if carries.is_none() && !names_this {
            return Ok(());
        }
        let mine = match crate::overlay_files::held(journal_dir, domain, actor, path).map_err(io)? {
            crate::overlay_files::Held::Nothing => return Ok(()),
            crate::overlay_files::Held::Tombstone => None,
            crate::overlay_files::Held::Bytes => Some(sha256_hex(
                &crate::overlay_files::read(journal_dir, domain, actor, path)
                    .map_err(io)?
                    .unwrap_or_default(),
            )),
        };
        let proposed = carries.map(|file| match file.change {
            crystalline_remote::state::ProposedChange::Deleted => None,
            _ => file.sha256.clone(),
        });
        let shared = match proposed {
            Some(holds) if holds == mine => Some(crate::draft_start::SharedFile {
                proposal: number,
                sha: mine,
            }),
            // Her file changed since it was staged: the branch holds what an
            // earlier share of this proposal recorded, when it still does.
            Some(holds)
                if names_this
                    && record.shared.as_ref().map(|shared| &shared.sha) == Some(&holds) =>
            {
                record.shared.clone()
            }
            _ => None,
        };
        if record.shared != shared {
            crate::draft_start::save_file(
                journal_dir,
                domain,
                actor,
                &crate::draft_start::DraftFileStart { shared, ..record },
            )
            .map_err(io)?;
        }
        Ok(())
    }

    /// Record what proposal `number` holds of `actor`'s row at `path`, which
    /// carries `file`, under the row's draft lock (see
    /// `Engine::record_what_the_proposal_holds`).
    #[allow(clippy::too_many_arguments)]
    async fn record_one_shared_row(
        &self,
        domain: &str,
        domain_id: DomainId,
        actor: &str,
        journal_dir: &Path,
        path: &str,
        file: &crystalline_remote::state::ProposedFile,
        number: u64,
    ) -> Result<()> {
        let lock = self.draft_lock(domain, actor, path)?;
        let _guard = lock.lock().await;
        let row = {
            let store = self.store.lock().await;
            store.overlay_entry(domain_id, actor, path).await?
        };
        let Some(row) = row else {
            return Ok(());
        };
        let record =
            crate::draft_start::load(journal_dir, domain, actor, path).map_err(|source| {
                EngineError::Io {
                    path: format!("the draft start record of '{path}'"),
                    source,
                }
            })?;
        let Some(record) = record else {
            return Ok(());
        };
        let holds = match file.sha256.as_deref() {
            Some(digest) => !row.tombstone && is_draft_digest(&row.content, digest),
            None => row.tombstone,
        };
        // This share carried the text an earlier share of the same proposal
        // recorded, and the row changed after it was staged: the branch holds
        // what was recorded then.
        let held_before = record.shared.as_ref().is_some_and(|before| {
            before.proposal == number
                && match (file.sha256.as_deref(), before.text.as_deref()) {
                    (Some(digest), Some(text)) => is_draft_digest(text, digest),
                    (None, None) => true,
                    _ => false,
                }
        });
        let shared = if holds {
            Some(crate::draft_start::SharedText {
                proposal: number,
                text: (!row.tombstone).then(|| crystalline_core::to_lf(&row.content).into_owned()),
            })
        } else if held_before {
            record.shared.clone()
        } else {
            None
        };
        if record.shared != shared {
            self.save_start(
                domain,
                actor,
                &crate::draft_start::DraftStart { shared, ..record },
            )?;
        }
        Ok(())
    }

    /// After a direct commit that deleted `deleted`: a draft of `actor`'s whose
    /// record starts at one of those paths starts nowhere now. She shared only
    /// that half of her move, so the deletion is hers and not the team's, and
    /// her draft is a new file whose next share adds it. Read against the record
    /// to the letter, as `from` is everywhere else.
    ///
    /// Best effort, like the rest of the commit's tail: the branch has moved and
    /// nothing here may turn that into an error. A record that cannot be read or
    /// saved stays, and the answer is a sentence for the receipt's `free_error`:
    /// the pass then reads that draft as `EditDelete`, as if the team had
    /// deleted the page, which refuses its share rather than losing anything,
    /// and she needs to know that the deletion was hers and the draft holds the
    /// only copy.
    async fn free_drafts_of_shared_deletions(
        &self,
        domain: &str,
        actor: &str,
        deleted: &[String],
    ) -> Option<String> {
        if deleted.is_empty() {
            return None;
        }
        let said = |e: &dyn std::fmt::Display| {
            let error = e.to_string().replace(['\n', '\r'], " ");
            format!(
                "Your deletion of {} reached the team, but a draft you moved from there could \
                 not be updated ({error}). It will look as if the team deleted the page. Do not \
                 discard that draft: it holds the only copy of the page.",
                deleted.join(", ")
            )
        };
        let mut failed: Option<String> = None;
        let records = self.journal_state_dir().and_then(|dir| {
            crate::draft_start::all(&dir, domain, actor)
                .map(|records| (dir, records))
                .map_err(|source| EngineError::Io {
                    path: format!("the draft start records of '{actor}' in '{domain}'"),
                    source,
                })
        });
        let (dir, records) = match records {
            Ok(read) => read,
            Err(e) => {
                tracing::warn!("reading the drafts a direct commit in '{domain}' freed: {e}");
                return Some(said(&e));
            }
        };
        for record in records {
            let Some(from) = record.from.as_deref() else {
                continue;
            };
            if from == record.path || !deleted.iter().any(|path| path == from) {
                continue;
            }
            let lock = match self.draft_lock(domain, actor, &record.path) {
                Ok(lock) => lock,
                Err(e) => {
                    tracing::warn!("freeing the draft '{}' in '{domain}': {e}", record.path);
                    failed.get_or_insert_with(|| said(&e));
                    continue;
                }
            };
            let _guard = lock.lock().await;
            // Read again under the lock: a writer may have moved it since.
            let freed = match crate::draft_start::load(&dir, domain, actor, &record.path) {
                Ok(Some(current)) if current.from.as_deref() == Some(from) => {
                    crate::draft_start::save(
                        &dir,
                        domain,
                        actor,
                        &crate::draft_start::DraftStart {
                            from: None,
                            start: None,
                            start_sha256: None,
                            ..current
                        },
                    )
                }
                Ok(_) => Ok(()),
                Err(e) => Err(e),
            };
            if let Err(e) = freed {
                tracing::warn!("freeing the draft '{}' in '{domain}': {e}", record.path);
                failed.get_or_insert_with(|| said(&e));
            }
        }
        failed
    }

    /// One actor's case-only moves in a domain: a tombstone at one spelling and
    /// a draft at another spelling of the same path. Sharing folds case, so
    /// these are the moves whose new spelling a share will not carry. The base
    /// snapshot tells a move from two real files: it holds the old spelling and
    /// not the new one.
    async fn case_only_moves(
        &self,
        domain: &str,
        actor: &str,
        state_dir: &Path,
    ) -> Result<Vec<Value>> {
        let base = crystalline_remote::state::OriginState::load(state_dir)?
            .map(|state| state.files)
            .unwrap_or_default();
        let store = self.store.lock().await;
        let Some(domain_id) = store.domain_id(domain).await? else {
            return Ok(Vec::new());
        };
        let entries = store.overlay_entries(domain_id, actor).await?;
        drop(store);
        let fold = crystalline_core::fold_path_case;
        Ok(entries
            .iter()
            .filter(|from| from.tombstone && base.contains_key(&from.path))
            .filter_map(|from| {
                entries
                    .iter()
                    .find(|to| {
                        !to.tombstone
                            && fold(&to.path) == fold(&from.path)
                            && !base.contains_key(&to.path)
                    })
                    .map(|to| json!({ "from": from.path, "to": to.path }))
            })
            .collect())
    }

    /// Take the team marker a share wrote into the folder back out when the
    /// share carried nothing to the team, so it never stays behind as a
    /// local change nobody asked for. Only while the MANIFEST still holds
    /// exactly what the marker write left: a pull or a person that changed
    /// it since owns it now. Best effort, like the write.
    async fn take_back_team_marker(&self, domain: &str, root: &Path, plan: &TeamMarker) {
        if !plan.in_folder {
            return;
        }
        let path = root.join(MANIFEST_PATH);
        let unchanged = std::fs::read_to_string(&path)
            .is_ok_and(|text| crystalline_core::to_lf(&text) == plan.marked);
        if !unchanged {
            return;
        }
        if let Err(e) = write_file(&path, &plan.current) {
            tracing::warn!("taking the team marker of '{domain}' back failed: {e}");
            return;
        }
        if let Err(e) = self.sync(Some(domain)).await {
            tracing::warn!("indexing '{domain}' after taking the team marker back failed: {e}");
        }
    }

    /// Refuse a share that would stack a layer on an open one while the domain
    /// reviews changes - in the words of whose that open proposal is.
    ///
    /// Two things a stacked layer needs are not there in review mode. A layer
    /// is detected against the CHAIN TIP, which may be another actor's open
    /// layer - a member would be proposing a change on top of work they never
    /// wrote and may not even be shown. And amending a layer replays the layers
    /// above it from the working tree, which in a reviewing domain holds no
    /// layer's content at all: the folder says what the team reviewed, and
    /// every layer lives in somebody's overlay.
    ///
    /// **So a reviewing domain carries one proposal at a time, and that costs
    /// two different refusals rather than one.** The proposal is this actor's
    /// own, and withdrawing it is a thing they may do
    /// ([`REVIEW_NO_STACKING`]); or it is somebody else's, and it is not
    /// theirs to withdraw, so the honest answer names whose it is and what to
    /// wait for ([`review_proposal_is_anothers`], an answer and not an error). One
    /// refusal for both would tell a member who has never shared anything to
    /// "share a fresh proposal instead", which is precisely what they were doing.
    ///
    /// **Whose it is comes from the journal's own record**, not from the forge
    /// record's `author_login`: that names the GitHub account a share's
    /// credential was connected as, and in the default instance identity mode
    /// every actor's share goes out on one login. A proposal opened before this
    /// was recorded, or by another machine, has no owner here and reads as
    /// somebody else's - the safe way round, since it never tells one actor to
    /// withdraw a proposal that is not theirs.
    ///
    /// This refusal is the actor's own proposal only, and only where stacks
    /// are served (probed once and recorded): with no stacks a member's own
    /// open proposal is updated in place, as it always was. Somebody else's
    /// open proposal is answered after the share's pull, whatever the forge
    /// serves ([`Engine::another_members_open_proposal`]).
    pub(super) fn refuse_open_proposal_while_reviewing(
        &self,
        domain: &str,
        state_dir: &Path,
        actor: &str,
    ) -> Result<()> {
        let Some(state) = crystalline_remote::state::OriginState::load(state_dir)
            .ok()
            .flatten()
            .filter(|state| state.stacks_available == Some(true))
        else {
            return Ok(());
        };
        let Some(open) = state
            .proposals
            .iter()
            .find(|p| p.status == crystalline_remote::state::ProposalStatus::Open)
        else {
            return Ok(());
        };
        if self.proposal_owner(domain, open.number).as_deref() == Some(actor) {
            return Err(EngineError::Refused(REVIEW_NO_STACKING.to_string()));
        }
        Ok(())
    }

    /// Whose proposal `number` is in review mode, from the journal's record.
    fn proposal_owner(&self, domain: &str, number: u64) -> Option<String> {
        let record = self
            .journal_state_dir()
            .map(|dir| crate::overlay_journal::journal_record(&dir, domain))
            .unwrap_or_default();
        record.proposals.get(&number.to_string()).cloned()
    }

    /// The open proposal of somebody else's that a share or a preview of
    /// `actor`'s in a reviewing domain waits for, with its owner when one is
    /// recorded.
    ///
    /// **A reviewing domain carries one proposal at a time, on every forge.**
    /// With no stacks a share would otherwise update the open proposal in
    /// place, and its branch would then hold only this actor's drafts: the
    /// owner's work would be gone from it, and once it merged her draft would
    /// read the team's text as her own share. So a share waits until that
    /// proposal is merged or closed.
    ///
    /// Read after the share's own pull, so a proposal the forge merged or
    /// closed since the last pull no longer stands in the way. A proposal
    /// whose owner is not recorded (opened before owners were recorded, or by
    /// another machine) is somebody else's: the safe way round.
    pub(super) fn another_members_open_proposal(
        &self,
        domain: &str,
        state_dir: &Path,
        actor: &str,
    ) -> Result<Option<(crystalline_remote::state::Proposal, Option<String>)>> {
        let Some(state) = crystalline_remote::state::OriginState::load(state_dir)? else {
            return Ok(None);
        };
        let Some(open) = state
            .proposals
            .into_iter()
            .find(|p| p.status == crystalline_remote::state::ProposalStatus::Open)
        else {
            return Ok(None);
        };
        let owner = self.proposal_owner(domain, open.number);
        if owner.as_deref() == Some(actor) {
            return Ok(None);
        }
        Ok(Some((open, owner)))
    }

    /// Indexes whatever the pull inside a share or a preview wrote to the
    /// working tree.
    ///
    /// Both `ops::propose` and `ops::propose_preview` pull first - freshness is
    /// part of proposing honestly - and a pull applies upstream files. Without
    /// this, those files sit on disk unsearchable until the poller's next tick
    /// happens to sync them, which is `origin_update_one`'s bug with a
    /// different call in front of it. Run unconditionally rather than only when
    /// something looks applied: the sync is incremental, so a pull that changed
    /// nothing costs a cheap no-op scan, and there is no cheaper signal here
    /// that is also correct (a preview reports the share's plan, not the pull's
    /// effect).
    ///
    /// Best effort in the same sense `origin_update_one`'s embed tail is: a
    /// failure is logged, never turned into a failed share whose proposal is
    /// already open on the forge.
    async fn index_what_the_share_pull_applied(&self, domain: &str, verb: &str) {
        if let Err(e) = self.sync(Some(domain)).await {
            tracing::warn!("syncing '{domain}' after {verb} failed: {e}");
            return;
        }
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after {verb} '{domain}' failed: {e}");
        }
    }

    /// Previews what a share of one domain would do, under its origin lock,
    /// without making a single provider write.
    ///
    /// The same three gates `origin_share` applies apply here: a preview runs
    /// the real pull first (freshness is part of previewing honestly), so it
    /// writes the working tree and is refused on a read-only instance exactly
    /// as a share is - and, for the same reason, it ends with the same sync and
    /// embed tail (see `Engine::index_what_the_share_pull_applied`).
    ///
    /// On a domain that reviews changes it also runs the convergence pass, as
    /// the share does. When its pull applied team changes, that pass is the
    /// pull's and converges every person's drafts, as an update would. When
    /// the pull applied nothing, it runs for the caller's own drafts only: a
    /// clean merge of hers is written into her rows, so the plan shows what her
    /// share would carry. A caller on the instance's credential
    /// (`ReadScopeFallback` taken) then writes no draft at all, and the plan
    /// names a stale draft of hers as a conflict until a pass has merged it.
    ///
    /// It carries the share's own credential resolution too, `actor` and all:
    /// a preview that resolved a different identity than the share would could
    /// promise a plan this instance then refuses to perform.
    ///
    /// `credential` is the one place that resolution bends, and only for the
    /// personal-token refusal (see [`PreviewCredential`]): a caller that asks
    /// for [`PreviewCredential::ReadScopeFallback`] gets the plan computed on
    /// the instance credential when the acting identity has connected none of
    /// its own, which is the browser's case - the checkbox list a person picks
    /// files in is fed by this call, so refusing it would make connecting a
    /// hoop in front of an unknown. Every other refusal stands for both
    /// callers, the acting login is `None` on that path (nothing personal was
    /// resolved to name), and the share itself still refuses.
    pub async fn origin_share_preview(
        &self,
        domain: &str,
        title: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        actor: ShareActor,
        credential: PreviewCredential,
    ) -> Result<Value> {
        self.origin_share_preview_with(domain, title, proposal, files, None, actor, credential)
            .await
    }

    /// [`Engine::origin_share_preview`] with `as_proposal`, the share's own
    /// choice of route on a `sharing: direct` domain (see
    /// [`Engine::origin_share_with`]), so the plan is the one that share
    /// would follow.
    #[allow(clippy::too_many_arguments)]
    pub async fn origin_share_preview_with(
        &self,
        domain: &str,
        title: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        as_proposal: Option<bool>,
        actor: ShareActor,
        credential: PreviewCredential,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // A preview only reads: it meets the read-only check, never the host
        // gate, which may claim a lock.
        self.refuse_read_only_flag(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // Read exactly where the share reads it, and for the same reason: off
        // the folder the team reviewed, never off the tree the preview detects
        // in (see `Engine::origin_share`).
        let sharing = crystalline_core::sharing_at(&root);
        let drafting = self.overlay_share_identity(domain, &actor)?;
        if let (Some(who), true) = (drafting.as_deref(), stacks_allowed) {
            // The preview carries the share's own gates, so nobody is asked to
            // confirm a share this instance would then refuse.
            self.refuse_open_proposal_while_reviewing(domain, &state_dir, who)?;
        }
        // A caller on the instance's credential only reads, so its preview
        // converges nobody's drafts.
        let mut pass = SharePass::Own;
        let (provider, login) = match self.resolve_share_provider(&actor, &spec.repo).await {
            Ok(resolved) => resolved,
            Err(e)
                if credential == PreviewCredential::ReadScopeFallback
                    && is_personal_token_missing(&e) =>
            {
                pass = SharePass::Nobody;
                (
                    self.resolve_origin_provider()
                        .map_err(|e| refusal_for(&actor, e))?,
                    None,
                )
            }
            Err(e) => return Err(e),
        };
        let acting = self.personal_write_login(login.as_deref());
        // The share's own staging, for the share's own reason: a preview that
        // planned against the folder would promise a share of files nobody
        // drafted (see `Engine::overlay_share_tree`). Its pass writes only the
        // caller's own drafts, since a preview is a question about those.
        let staging = self
            .overlay_share_tree(
                domain,
                drafting.as_deref(),
                provider.as_ref(),
                (&spec, &root, &state_dir),
                acting.as_deref(),
                ops::BranchCleanup::Defer,
                pass,
            )
            .await?;
        // The share's own wait for somebody else's open proposal, read after
        // the same pull, and answered in the plan so the changes still show.
        let anothers = match drafting.as_deref() {
            Some(who) => self.another_members_open_proposal(domain, &state_dir, who)?,
            None => None,
        };
        // Pinned exactly as the share pins it, and for the same reason: a
        // preview's own inline pull would merge into the staged tree too.
        let pinned = staging.as_ref().map(|prepared| {
            share_staging::PinnedHead::new(provider.as_ref(), prepared.pinned.clone())
        });
        let preview_provider: &dyn Provider = match &pinned {
            Some(pinned) => pinned,
            None => provider.as_ref(),
        };
        let detect_in = staging
            .as_ref()
            .map_or(root.as_path(), |prepared| prepared.staging.root());
        // The share's own gate, read at the same point (see
        // `Engine::overlay_share_tree`), and answered in the plan rather than
        // as an error, so the changes still show beside it.
        let conflicts = staging
            .as_ref()
            .map(|prepared| prepared.conflicts.clone())
            .unwrap_or_default();
        let mut plan = ops::propose_preview(
            preview_provider,
            &spec,
            detect_in,
            domain,
            &state_dir,
            ops::ShareOptions {
                as_proposal,
                title,
                description: None,
                note: None,
                proposal,
                stacks_allowed,
                // Carried for the same reason the provider is: a preview
                // resolves exactly what the share would. It records nothing.
                author_login: login.as_deref(),
                files,
                sharing,
                // The mode in effect: a pending switch keeps sharing the
                // engrams still under `reflections/` until they move out.
                reflection: self.reflection_mode(domain).await?,
            },
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        .map_err(|e| enrich_write_error(e, acting.as_deref(), &spec.repo))?;
        self.index_what_the_share_pull_applied(domain, "previewing a share")
            .await;
        // Provenance preselection is a guess at whose work a mixed delta holds,
        // and in review mode there is nothing left to guess: every path in the
        // plan is one of this actor's own drafts. So the plan is served without
        // it rather than with a column answering a question nobody asked.
        let provenance = drafting.is_none().then_some(root.as_path());
        // Named files with a conflict among them: the share would refuse, so the
        // plan's action is that refusal, and the keys it carries follow from it
        // as for any other action. With no files the plan stays the ordinary
        // one and names her conflicts beside it: Fluid asks with no files and
        // then lets her tick, and a conflicted file only takes its box away.
        let blocking = match files {
            Some(_) => draft_conflicts::conflicts_block(&conflicts, files),
            None => Vec::new(),
        };
        if !blocking.is_empty() {
            plan.action = ops::PlannedAction::ConflictsPending {
                count: blocking.len(),
            };
        }
        if let Some((open, _)) = &anothers {
            plan.action = ops::PlannedAction::ProposalOpen {
                number: open.number,
                url: open.url.clone(),
                title: open.title.clone(),
                own: false,
            };
        }
        let mut plan = origin::share_plan_json(&plan, provenance);
        if let Some((open, owner)) = &anothers {
            plan["guidance"] = json!(review_proposal_is_anothers(owner.as_deref(), open.number));
        }
        if !blocking.is_empty() {
            plan["conflicts"] = origin::draft_conflicts_json(&blocking);
        } else if files.is_none() && !conflicts.is_empty() {
            plan["draft_conflicts"] = origin::draft_conflicts_json(&conflicts);
        }
        // The marker the share would add, named beside the plan rather than
        // in its change list: the folder does not hold it yet, and the
        // change list is what a person ticks and discards.
        if let Ok(Some(_)) = self
            .plan_team_marker(domain, detect_in, &state_dir, files)
            .await
        {
            plan["team_marker"] = team_marker_json();
        }
        // The repository beside the branch, so Fluid's share dialog can say
        // where a commit goes without a second lookup.
        plan["repo"] = json!(spec.repo);
        // The same word the share's receipt will carry, so the dialog does not
        // say the team has everything before the share either.
        if let Some(who) = drafting.as_deref() {
            let moves = self.case_only_moves(domain, who, &state_dir).await?;
            note_unshared_spelling(&mut plan, &moves);
        }
        Ok(plan)
    }

    /// Withdraws a share proposal for one domain: closes its pull request on
    /// the forge, retires its branch (deleted unless an open pull request is
    /// based on it or comes from it, then kept and named in `kept_branches`),
    /// optionally restores the shared files (`revert`) and records it as
    /// withdrawn. Under the
    /// domain's origin lock; syncs and embeds afterward only when files
    /// moved. Refuses when collaboration is off and on a read-only instance.
    ///
    /// `actor` decides the credential the close and the branch delete go out
    /// on, exactly as it does for a share.
    pub async fn origin_withdraw(
        &self,
        domain: &str,
        proposal: Option<u64>,
        revert: bool,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // Whose withdrawal this is, on a domain that reviews changes - and the
        // refusal an agent with no identity gets, in the words every draft verb
        // refuses in. `None` is a domain that takes changes directly, where a
        // revert is the working-tree restore it always was.
        let drafting = self.overlay_share_identity(domain, &actor)?;
        // What the chain holds BEFORE the withdrawal, so an overlay revert can
        // find the record it is undoing. Read here and not back off the saved
        // state afterwards: the ordinary path settles the record into
        // `history`, which `OriginState::push_history` caps at twenty, so a
        // busy domain can evict the very proposal this call just withdrew - and
        // a revert that then found nothing would quietly restore nothing while
        // the receipt said the withdrawal had worked.
        let open_before: Vec<crystalline_remote::state::Proposal> = if drafting.is_some() && revert
        {
            crystalline_remote::state::OriginState::load(&state_dir)?
                .map(|state| state.proposals)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let (provider, login) = self.resolve_share_provider(&actor, &spec.repo).await?;
        let acting = self.personal_write_login(login.as_deref());
        let mut report = ops::withdraw(
            provider.as_ref(),
            &spec,
            &root,
            &state_dir,
            proposal,
            // A revert of a reviewing domain never reaches the folder: what
            // the withdrawal undoes is the actor's own overlay, and the folder
            // on disk is what the team reviewed and nobody withdrew.
            revert && drafting.is_none(),
            stacks_allowed,
            // The mode in effect now, at the withdrawal: a layer shared while
            // a switch was pending is reverted in full as long as the switch
            // is still pending. Once it is in effect, the revert leaves the
            // files under `reflections/` alone.
            self.reflection_mode(domain).await?,
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        .map_err(|e| enrich_write_error(e, acting.as_deref(), &spec.repo))?;

        if drafting.is_some() {
            // The proposal is gone from the chain, so whose it was is no longer
            // a question anything asks.
            self.record_proposal_actor(domain, report.number, None);
        }
        if let (Some(who), true) = (drafting.as_deref(), revert) {
            self.revert_into_overlay(domain, who, &open_before, &mut report)
                .await?;
            // Nothing on disk moved, so there is nothing to sync: the overlay
            // writes nudged the embedder themselves.
            return Ok(origin::withdraw_report_json(&report));
        }

        if !report.restored.is_empty() || !report.deleted.is_empty() {
            self.sync(Some(domain)).await?;
            if !self.request_embed()
                && let Err(e) = self.embed_pending().await
            {
                tracing::warn!(
                    "embedding after withdrawing proposal #{} for '{domain}' failed: {e}",
                    report.number
                );
            }
        }
        Ok(origin::withdraw_report_json(&report))
    }

    /// Puts one actor's own view back the way it stood before the withdrawn
    /// proposal was shared: every path that proposal carried stops being their
    /// draft.
    ///
    /// The undo is a clear and never a write, and that follows from what a
    /// review-mode share is. A share of a reviewing domain is made ENTIRELY of
    /// the acting actor's overlay entries, so the state before it was "this
    /// actor held nothing at these paths" - a proposed addition, a proposed
    /// rewrite and a proposed deletion all go back to the same thing, which is
    /// the base showing through again.
    ///
    /// **It reverts the ACTING actor's overlay, whosever the proposal was.**
    /// Nothing in this task restricts a withdrawal to the proposal's author, so
    /// an actor withdrawing somebody else's proposal finds none of its paths in
    /// their own overlay and reverts nothing - an empty `restored` and
    /// `deleted` beside a withdrawal that did happen. That is the honest
    /// outcome of the two rules meeting (a revert is per actor, a withdrawal is
    /// not) and not a silent failure of either; who may withdraw whose proposal
    /// is a question the program has not answered yet.
    ///
    /// **A draft edited since it was shared is never touched**, the rule
    /// [`ops::withdraw`]'s own revert keeps for a file: the recorded digest is
    /// what says whether what stands here is still what was proposed, and
    /// anything else is newer work. Such a path is named in `skipped_diverged`
    /// exactly as the folder path would be.
    ///
    /// The one case a revert would have to WRITE rather than clear - a path a
    /// lower layer of a stack added and this layer changed, whose pre-share
    /// content is that layer's blob - cannot arise here, because stacking on an
    /// open layer is refused outright while a domain reviews
    /// ([`Engine::origin_share`]). If stacks ever reach review mode, this is
    /// the half that has to learn to write.
    async fn revert_into_overlay(
        &self,
        domain: &str,
        actor: &str,
        open_before: &[crystalline_remote::state::Proposal],
        report: &mut crystalline_remote::ops::WithdrawReport,
    ) -> Result<()> {
        // The record as it stood before the withdrawal resolved its own target,
        // matched on the number the withdrawal reports.
        let Some(withdrawn) = open_before.iter().find(|p| p.number == report.number) else {
            return Ok(());
        };
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let Some(domain_id) = domain_id else {
            return Ok(());
        };
        for file in &withdrawn.files {
            let held = {
                let store = self.store.lock().await;
                store.overlay_entry(domain_id, actor, &file.path).await?
            };
            let Some(held) = held else {
                continue;
            };
            // What was proposed, against what this actor holds now. A
            // tombstone proposed a deletion and carries no content of its own,
            // so its digest is the absence the proposal recorded.
            let unchanged = match file.sha256.as_deref() {
                Some(proposed) => !held.tombstone && is_draft_digest(&held.content, proposed),
                None => held.tombstone,
            };
            if !unchanged {
                report.skipped_diverged.push(file.path.clone());
                continue;
            }
            DomainView::for_actor(self, domain, &HashSet::new(), actor)?
                .drop(domain_id, &file.path)
                .await?;
            match file.change {
                // A page only this actor had goes away with the proposal.
                crystalline_remote::state::ProposedChange::Added => {
                    report.deleted.push(file.path.clone());
                }
                // A rewrite or a deletion of a page the team has: the team's
                // own version shows through again.
                _ => report.restored.push(file.path.clone()),
            }
        }
        Ok(())
    }

    /// What one actor holds in a reviewing domain, rows and files, at the
    /// spelling and in the order the change list reports: `(path, kind,
    /// current bytes)` with `None` bytes for a tombstone, sorted by path.
    ///
    /// The kind is the folder's answer: `deleted` for a tombstone, `modified`
    /// where the folder holds the path, `added` where it does not. That is
    /// exactly the set a review-mode share stages
    /// ([`crate::share_staging`]), so the list and the share agree.
    pub(super) async fn overlay_local_changes(
        &self,
        domain: &str,
        root: &Path,
        actor: &str,
    ) -> Result<Vec<(String, &'static str, Option<Vec<u8>>)>> {
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let mut out: Vec<(String, &'static str, Option<Vec<u8>>)> = Vec::new();
        if let Some(domain_id) = domain_id {
            let rows = {
                let store = self.store.lock().await;
                store.overlay_entries(domain_id, actor).await?
            };
            for row in rows {
                if !origin::takes_part_in_local_change(&row.path) {
                    continue;
                }
                let kind = if row.tombstone {
                    "deleted"
                } else if root.join(&row.path).is_file() {
                    "modified"
                } else {
                    "added"
                };
                let bytes = (!row.tombstone).then(|| row.content.into_bytes());
                out.push((row.path, kind, bytes));
            }
        }
        let state_dir = self.journal_state_dir()?;
        for entry in crate::overlay_files::entries(&state_dir, domain, actor).entries {
            if !origin::takes_part_in_local_change(&entry.path) {
                continue;
            }
            let kind = if entry.tombstone {
                "deleted"
            } else if root.join(&entry.path).is_file() {
                "modified"
            } else {
                "added"
            };
            let bytes = if entry.tombstone {
                None
            } else {
                crate::overlay_files::read(&state_dir, domain, actor, &entry.path).map_err(
                    |source| EngineError::Io {
                        path: entry.path.clone(),
                        source,
                    },
                )?
            };
            out.push((entry.path, kind, bytes));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// The folder's own bytes at a review-mode draft's path, or `None` where
    /// the folder holds nothing there. Only ever asked for a path the actor's
    /// overlay already names, which is what keeps the join off the raw
    /// request: every such path was validated by the verb that wrote it.
    fn folder_side(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(root.join(path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(EngineError::Io {
                path: path.to_string(),
                source,
            }),
        }
    }

    /// The team-domain walk every method below shares: the origin state, the
    /// base it detects against and the detected delta. Under no lock; a read
    /// a beat stale is the promise every offline read here makes.
    /// `reflection` is the domain's Reflection mode in effect
    /// ([`Engine::reflection_mode`]), which a plain `fn` cannot read itself.
    pub(super) fn team_local_changes(
        &self,
        domain: &str,
        reflection: crystalline_core::ReflectionMode,
    ) -> Result<TeamChanges> {
        #[cfg(any(test, feature = "testing"))]
        self.detection_walks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let state = crystalline_remote::state::OriginState::load(&state_dir)?.ok_or_else(|| {
            EngineError::Invalid(format!("domain '{domain}' has no origin state"))
        })?;
        let base = ops::unshared_base(&state);
        let local = crystalline_remote::changes::detect_local_changes_against(
            &root, &base, &state_dir, reflection,
        )?;
        Ok((root, state_dir, base, local))
    }

    /// The domain's own refusal for a caller that asked about local changes
    /// where there are none to have: a domain with no team origin shares
    /// nothing, so nothing of it is unshared.
    fn local_changes_need_an_origin(&self, domain: &str) -> Result<()> {
        if !self.domain_has_origin(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain '{domain}' has no team origin; only a team domain has local changes"
            )));
        }
        Ok(())
    }

    /// Every unshared change of `domain`, offline: `{ domain, mode, changes,
    /// skipped_large }`. A team domain answers the working tree against
    /// `unshared_base`, the comparison a share makes; a reviewing domain
    /// answers the acting actor's own drafts against the folder, and an
    /// anonymous HTTP agent, who holds no draft there, an empty list. Never
    /// pulls, never probes, never resolves a provider.
    pub async fn local_changes(&self, domain: &str, actor: &ShareActor) -> Result<Value> {
        self.local_changes_listing(domain, actor, false).await
    }

    /// The same envelope as [`Engine::local_changes`] with both sides of every
    /// change inlined: each entry is what [`Engine::local_change`] answers for
    /// that path, uncapped, and in the same order.
    ///
    /// One detection walk for the whole envelope, which is the point of it:
    /// asking [`Engine::local_change`] per listed path would re-run the walk
    /// (a read and hash of every file in the domain, or a reviewing domain's
    /// overlay read) once per change, so a domain with a hundred unshared
    /// changes paid for a hundred walks to answer one call.
    #[doc(hidden)]
    pub async fn local_changes_detailed(&self, domain: &str, actor: &ShareActor) -> Result<Value> {
        self.local_changes_listing(domain, actor, true).await
    }

    /// The body both listings share. `sides` decides only what each entry
    /// carries: the summary row, or that row with both texts on it.
    async fn local_changes_listing(
        &self,
        domain: &str,
        actor: &ShareActor,
        sides: bool,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        self.local_changes_need_an_origin(domain)?;
        let entry = |path: &str, kind: &str, base: Option<&[u8]>, current: Option<&[u8]>| {
            if sides {
                // Uncapped, because a caller deciding what to discard reads
                // the whole file rather than a preview of it.
                let mut value = origin::change_detail_json(path, kind, base, current, None);
                value["domain"] = json!(domain);
                value
            } else {
                origin::change_entry_json(path, kind, base, current)
            }
        };
        if self.reviews_changes(domain) {
            let Ok(who) = share_staging::overlay_share_actor(actor) else {
                return Ok(
                    json!({ "domain": domain, "mode": "review", "changes": [], "skipped_large": [] }),
                );
            };
            let (_, root, _) = self.origin_spec_for_domain(domain)?;
            let mut changes = Vec::new();
            for (path, kind, current) in self.overlay_local_changes(domain, &root, &who).await? {
                let base = Self::folder_side(&root, &path)?;
                changes.push(entry(&path, kind, base.as_deref(), current.as_deref()));
            }
            return Ok(
                json!({ "domain": domain, "mode": "review", "changes": changes, "skipped_large": [] }),
            );
        }
        let (root, state_dir, _, local) =
            self.team_local_changes(domain, self.reflection_mode(domain).await?)?;
        let mut changes = Vec::new();
        for change in local.substantive() {
            let Some(found) = ops::local_change_sides(&root, &state_dir, &local, change.path())?
            else {
                continue;
            };
            changes.push(entry(
                change.path(),
                change_kind(change),
                found.base.as_deref(),
                found.current.as_deref(),
            ));
        }
        let skipped: Vec<Value> = local
            .skipped_large
            .iter()
            .map(|(path, size)| json!({ "path": path, "size": size }))
            .collect();
        Ok(
            json!({ "domain": domain, "mode": "team", "changes": changes, "skipped_large": skipped }),
        )
    }

    /// Both sides of every unshared change of `domain`, in the order
    /// [`Engine::local_changes`] reports them: what `origin_status`'s `diff`
    /// block carries: the changes array of
    /// [`Engine::local_changes_detailed`]'s envelope, so the whole block costs
    /// the one walk that listing makes.
    async fn local_change_sides(&self, domain: &str, actor: &ShareActor) -> Result<Value> {
        let mut listed = self.local_changes_detailed(domain, actor).await?;
        Ok(listed["changes"].take())
    }

    /// Both sides of one unshared change, or `NotFound` in the words
    /// `select_share_files` refuses an unknown path in. `cap` bounds a text
    /// side; `None` answers every text.
    pub async fn local_change(
        &self,
        domain: &str,
        path: &str,
        actor: &ShareActor,
        cap: Option<usize>,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        self.local_changes_need_an_origin(domain)?;
        let not_a_change = || {
            EngineError::NotFound(format!(
                "not among this domain's unshared changes: {path}; take the paths from the change list, which names every file that differs"
            ))
        };
        if self.reviews_changes(domain) {
            let who = share_staging::overlay_share_actor(actor)?;
            let (_, root, _) = self.origin_spec_for_domain(domain)?;
            let found = self
                .overlay_local_changes(domain, &root, &who)
                .await?
                .into_iter()
                .find(|(p, _, _)| p == path)
                .ok_or_else(not_a_change)?;
            let (path, kind, current) = found;
            let base = Self::folder_side(&root, &path)?;
            let mut value =
                origin::change_detail_json(&path, kind, base.as_deref(), current.as_deref(), cap);
            value["domain"] = json!(domain);
            return Ok(value);
        }
        let (root, state_dir, _, local) =
            self.team_local_changes(domain, self.reflection_mode(domain).await?)?;
        let sides =
            ops::local_change_sides(&root, &state_dir, &local, path)?.ok_or_else(not_a_change)?;
        let mut value = origin::change_detail_json(
            sides.change.path(),
            change_kind(&sides.change),
            sides.base.as_deref(),
            sides.current.as_deref(),
            cap,
        );
        value["domain"] = json!(domain);
        Ok(value)
    }

    /// Put the named paths back the way the team has them: `{ domain,
    /// restored, deleted, cleared: [{ path, kind }], refused: [{ path, reason }],
    /// reindexed }`. Under the domain's origin lock like a withdrawal, so it
    /// never interleaves with a share's pull; refuses on a read-only instance.
    /// A team domain restores from the base copy and re-indexes exactly the
    /// touched paths; a reviewing domain clears the acting actor's own drafts
    /// through `DomainView::drop` and `overlay_files::clear` and moves
    /// nothing in the folder. One refused path never stops the others.
    pub async fn discard_local_changes(
        &self,
        domain: &str,
        targets: &[DiscardTarget],
        actor: &ShareActor,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.local_changes_need_an_origin(domain)?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        // A discard puts the domain's files back: only its host does that,
        // refused before any file is touched.
        self.require_file_host(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        let drafting = self.overlay_share_identity(domain, actor)?;
        if let Some(who) = drafting {
            return self.discard_into_overlay(domain, &who, targets).await;
        }
        let (root, state_dir, base, local) =
            self.team_local_changes(domain, self.reflection_mode(domain).await?)?;
        let targets = fill_unguarded(targets, &local);
        let report = ops::discard_local_files(&root, &state_dir, &base, &local, &targets)?;
        let touched: Vec<String> = report
            .restored
            .iter()
            .chain(report.deleted.iter())
            .map(|p| local.disk_path(p).to_string())
            .collect();
        let reindexed = touched.len();
        if !touched.is_empty() {
            // The targeted pass the import already takes: exactly these paths,
            // which is also what rebuilds a folder's listing beside a discarded
            // engram in a domain that shares its listings.
            self.sync_paths_as(domain, touched, share_actor_label(actor).as_deref())
                .await?;
            if !self.request_embed()
                && let Err(e) = self.embed_pending().await
            {
                tracing::warn!("embedding after discarding changes in '{domain}' failed: {e}");
            }
        }
        let refused: Vec<Value> = report
            .refused
            .iter()
            .map(|(path, reason)| json!({ "path": path, "reason": reason.code() }))
            .collect();
        Ok(json!({
            "domain": domain,
            "restored": report.restored,
            "deleted": report.deleted,
            "cleared": [],
            "refused": refused,
            "reindexed": reindexed,
        }))
    }

    /// The review-mode half of [`Engine::discard_local_changes`]: a clear and
    /// never a write, for the reason [`Engine::revert_into_overlay`] gives.
    /// A draft somebody has open in a live editor is refused rather than
    /// closed under them, since the room's own save would write it straight
    /// back.
    async fn discard_into_overlay(
        &self,
        domain: &str,
        actor: &str,
        targets: &[DiscardTarget],
    ) -> Result<Value> {
        let (_, root, _) = self.origin_spec_for_domain(domain)?;
        let state_dir = self.journal_state_dir()?;
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let mut cleared: Vec<Value> = Vec::new();
        let mut refused: Vec<Value> = Vec::new();
        for target in targets {
            let path = target
                .path
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();
            // A generated listing is never a change of this feature, so it is
            // never a path a discard can name, whatever the MANIFEST says
            // about sharing listings. The team arm gets this from
            // `resolve_local_change`; here it is the same rule written out,
            // rather than something that happens to hold because no verb
            // writes such a row today.
            if !origin::takes_part_in_local_change(&path) {
                refused.push(json!({ "path": path, "reason": "unknown_path" }));
                continue;
            }
            let row = match domain_id {
                Some(domain_id) => {
                    let store = self.store.lock().await;
                    store.overlay_entry(domain_id, actor, &path).await?
                }
                None => None,
            };
            if let (Some(domain_id), Some(held)) = (domain_id, row) {
                // No digest is "discard what the list showed me", the way an
                // agent calling without `expected` means it; a digest is the
                // guard that newer work is never dropped.
                // The digest is of the row's own content, the way
                // [`Engine::revert_into_overlay`] takes it and the way the
                // change list reports it: a row's stamp column carries
                // whatever the write recorded there, which is a fact about a
                // file the draft may not have.
                let unchanged = match target.sha256.as_deref() {
                    Some(expected) => !held.tombstone && is_draft_digest(&held.content, expected),
                    None => true,
                };
                if !unchanged {
                    refused.push(json!({ "path": path, "reason": "changed_since" }));
                    continue;
                }
                let open = match self.collab_rooms() {
                    Some(rooms) => {
                        rooms
                            .has_live_room(domain, &held.permalink, Some(actor))
                            .await
                    }
                    None => false,
                };
                if open {
                    refused.push(json!({ "path": path, "reason": "open_in_editor" }));
                    continue;
                }
                let kind = if held.tombstone {
                    "deleted"
                } else if root.join(&path).is_file() {
                    "modified"
                } else {
                    "added"
                };
                DomainView::for_actor(self, domain, &HashSet::new(), actor)?
                    .drop(domain_id, &path)
                    .await?;
                cleared.push(json!({ "path": path, "kind": kind }));
                continue;
            }
            let io = |source: std::io::Error| EngineError::Io {
                path: path.clone(),
                source,
            };
            // A path the files overlay will not even address holds nothing
            // there, which is the same answer as an empty folder: the caller
            // named something this domain has no change at. A refusal to
            // address is not a filesystem failure, so it is answered rather
            // than raised.
            let held = match crate::overlay_files::held(&state_dir, domain, actor, &path) {
                Ok(held) => held,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                    refused.push(json!({ "path": path, "reason": "unknown_path" }));
                    continue;
                }
                Err(e) => return Err(io(e)),
            };
            match held {
                crate::overlay_files::Held::Nothing => {
                    refused.push(json!({ "path": path, "reason": "unknown_path" }));
                }
                crate::overlay_files::Held::Bytes => {
                    let bytes = crate::overlay_files::read(&state_dir, domain, actor, &path)
                        .map_err(io)?
                        .unwrap_or_default();
                    if target
                        .sha256
                        .as_deref()
                        .is_some_and(|expected| expected != sha256_hex(&bytes))
                    {
                        refused.push(json!({ "path": path, "reason": "changed_since" }));
                        continue;
                    }
                    crate::overlay_files::clear(&state_dir, domain, actor, &path).map_err(io)?;
                    let kind = if root.join(&path).is_file() {
                        "modified"
                    } else {
                        "added"
                    };
                    cleared.push(json!({ "path": path, "kind": kind }));
                }
                crate::overlay_files::Held::Tombstone => {
                    // A deletion's current side is absence, so a caller that
                    // looked at bytes was looking at something else.
                    if target.sha256.is_some() {
                        refused.push(json!({ "path": path, "reason": "changed_since" }));
                        continue;
                    }
                    crate::overlay_files::clear(&state_dir, domain, actor, &path).map_err(io)?;
                    cleared.push(json!({ "path": path, "kind": "deleted" }));
                }
            }
        }
        Ok(json!({
            "domain": domain,
            "restored": [],
            "deleted": [],
            "cleared": cleared,
            "refused": refused,
            "reindexed": 0,
        }))
    }

    /// One conflict's full detail: both recorded sides plus the current local
    /// content, addressed by id or by path (exactly one must be given; if
    /// both arrive the id wins and the path is ignored, never mixed, so an id
    /// lookup can never be answered by some other conflict that happens to
    /// match the path). Neither is `EngineError::Invalid` rather than a
    /// misleading not-found. Sides are returned as UTF-8 strings; a side that
    /// exists but is not UTF-8 comes back null with `note` saying so. A pure
    /// read: no gate beyond the domain being registered with an origin, no
    /// lock needed.
    pub async fn origin_conflict_detail(
        &self,
        domain: &str,
        id: Option<&str>,
        path: Option<&str>,
    ) -> Result<Value> {
        if id.is_none() && path.is_none() {
            return Err(EngineError::Invalid(
                "origin_conflict_detail needs an id or a path".to_string(),
            ));
        }
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let state = crystalline_remote::state::OriginState::load(&state_dir)?.ok_or_else(|| {
            EngineError::Invalid(format!("domain '{domain}' has no origin state"))
        })?;
        let conflict = state
            .conflicts
            .iter()
            .find(|c| match (id, path) {
                (Some(id), _) => c.id == id,
                (None, Some(path)) => c.path == path,
                (None, None) => false,
            })
            .cloned()
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no open conflict {} for '{domain}'",
                    id.or(path).unwrap_or("(none named)")
                ))
            })?;
        let (base, upstream) =
            crystalline_remote::state::read_conflict_files(&state_dir, &conflict.id)?;
        let local_path = root.join(&conflict.path);
        let local = match std::fs::read(&local_path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(EngineError::Io {
                    path: local_path.display().to_string(),
                    source: e,
                });
            }
        };
        let mut note: Option<String> = None;
        let base_v = utf8_side(base, "base", &mut note);
        let local_v = utf8_side(local, "local", &mut note);
        let upstream_v = utf8_side(upstream, "upstream", &mut note);
        Ok(json!({
            "id": conflict.id,
            "path": conflict.path,
            "kind": conflict.kind,
            "detected_at": conflict.detected_at,
            "base": base_v,
            "local": local_v,
            "upstream": upstream_v,
            "note": note,
        }))
    }

    /// Resolves one recorded conflict for one domain, under its origin lock,
    /// then syncs the domain (and embeds) since resolving writes the
    /// working tree.
    ///
    /// `keep` is `"mine"` or `"theirs"`; exactly one of `keep` or `content`
    /// must be supplied (see `origin::resolution_from`). Refuses with
    /// `github.enabled`'s message when collaboration is off, and with
    /// `EngineError::ReadOnly` on a read-only instance. A draft in a domain
    /// that reviews changes settles with collaboration off: that makes no call
    /// to the forge, and leaving review needs every draft settled.
    ///
    /// `actor` makes no provider call of its own - resolving writes this
    /// machine and reaches the forge later, on the next share, under whoever
    /// performs that. What it decides is WHOSE the resolution is: on a domain
    /// that reviews changes the conflict being settled is one actor's draft
    /// standing against a base that moved under it, so the resolution joins
    /// that actor's overlay and never the folder the team reviewed. An agent
    /// with no identity is refused there in the words every draft verb refuses
    /// in; on a domain that takes changes directly the identity changes
    /// nothing, exactly as before.
    pub async fn origin_resolve(
        &self,
        domain: &str,
        path: &str,
        keep: Option<&str>,
        content: Option<&[u8]>,
        actor: ShareActor,
    ) -> Result<Value> {
        // A draft settles with collaboration off, so only a domain that takes
        // changes directly is refused here. Its folder conflict is refused
        // again below, for a reviewing domain too.
        if !self.config.read().unwrap().github_enabled() && !self.drafts_stay_open(domain) {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // Taking the team's side is what a pull does, so it stays allowed on a
        // read-only domain; keeping your own text or a merge would change it.
        if keep != Some("theirs") {
            self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
                .await?;
        }
        // A draft file is settled here as a page is, from its own record. It
        // is never merged. Under the origin lock, as a page is, so a resolve
        // waits out a fold's locked window.
        if is_assets_reserved(path) && self.drafts_stay_open(domain) {
            let who = share_staging::overlay_share_actor(&actor)?;
            let resolution = origin::resolution_from(keep, content)?;
            let lock = self.origin_lock_registered(domain)?;
            let _guard = lock.lock().await;
            return self
                .resolve_file_in_overlay(domain, &who, path, resolution)
                .await;
        }
        // An attachment is not an engram, and in a domain that takes changes
        // directly this verb settles engrams. The path would be normalized to
        // `<path>.md` on the next line and then miss everything, so the
        // refusal comes first and says what to do instead: the bytes are
        // settled by writing them again or by deleting them, which is the same
        // pair of verbs that put them there.
        if is_assets_reserved(path) {
            return Err(EngineError::Invalid(format!(
                "'{path}' is an attachment, and a conflict resolution settles an engram's \
                 markdown; upload the file again to keep your version or delete it to take the \
                 one the team has"
            )));
        }
        let resolution = origin::resolution_from(keep, content)?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // One spelling from here down. A caller who names `notes/plan` and a
        // caller who names `notes/plan.md` mean one engram, and the draft
        // lookup, the recorded-conflict lookup and `ops::resolve` have to be
        // asked about the same one or a missing suffix would turn a resolution
        // into a refusal.
        let path = &normalize_md(path);
        // Keeping your own MANIFEST text or a merge may switch read_only on,
        // which is the owner's (decision D10). The machine owner may; this
        // verb carries no scope for anybody else, so for them an off-to-on
        // result is refused and the owner switches it in the domain policies
        // once the conflict is settled. Taking the team's side is a pull and
        // is never checked.
        if is_manifest_path(path) && keep != Some("theirs") && !matches!(&actor, ShareActor::Owner)
        {
            let after = match content {
                Some(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                None => std::fs::read_to_string(root.join(path)).unwrap_or_default(),
            };
            if self.switches_read_only_on(domain, &after).await {
                return Err(EngineError::Forbidden(
                    "resolving a conflict does not switch read_only on; the owner switches it in the domain policies"
                        .to_string(),
                ));
            }
        }
        // A fold that stopped part way leaves the drafts settling as in review
        // mode, so the fold can be repeated once each one is settled.
        let drafting = match self.drafts_stay_open(domain) {
            true => Some(share_staging::overlay_share_actor(&actor)?),
            false => None,
        };
        if let Some(who) = drafting {
            let held = {
                let store = self.store.lock().await;
                match store.domain_id(domain).await? {
                    Some(domain_id) => {
                        Some((domain_id, store.overlay_entry(domain_id, &who, path).await?))
                    }
                    None => None,
                }
            };
            match held {
                Some((domain_id, Some(_))) => {
                    return self
                        .resolve_in_overlay(domain, domain_id, &who, path, resolution, &state_dir)
                        .await;
                }
                // Nothing of this caller's stands here. A conflict the pull
                // recorded is the FOLDER's and never anybody's draft: it got
                // there because the reviewed folder was edited out of band and
                // upstream then changed the same file, and settling it is what
                // puts the folder back level with its own base. So it is
                // settled the way it always was, on the folder - review mode is
                // not a reason to leave a conflict standing in the team's own
                // files with no verb that can reach it. The overlay rows are
                // untouched by that path: a sync writes the base dimension and
                // a draft is somebody else's row entirely.
                _ if !self.folder_conflict_at(&state_dir, path)? => {
                    return Err(EngineError::NotFound(format!(
                        "you are not drafting '{path}' in domain '{domain}', and no conflict \
                         stands there either. This domain reviews changes, so what a resolution \
                         settles is your own draft: draft the change first, then settle it"
                    )));
                }
                _ => {}
            }
        }
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        let report = ops::resolve(&root, &state_dir, path, resolution)?;

        self.sync(Some(domain)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after resolving a conflict for '{domain}' failed: {e}");
        }

        Ok(json!({
            "resolved": report.resolved,
            "remaining": report.remaining,
        }))
    }

    /// Settles one actor's draft file beside the pages. A file never merges.
    ///
    /// - **Keep mine** leaves her bytes or her deletion and moves the start to
    ///   the team's digest now, so hers replaces the team's when she shares. A
    ///   team file that is gone leaves no start, so her file lands as a new one.
    /// - **Take theirs** ends the draft file and its record.
    /// - **Merged** is refused: there is no text to merge.
    ///
    /// Under the file's write lock, the one an upload takes, so an upload and a
    /// resolve of one file never cross. The caller holds the origin lock.
    async fn resolve_file_in_overlay(
        &self,
        domain: &str,
        actor: &str,
        path: &str,
        resolution: ops::Resolution<'_>,
    ) -> Result<Value> {
        super::validate_attachment_path(path)?;
        if matches!(resolution, ops::Resolution::Merged(_)) {
            return Err(EngineError::Invalid(
                "A file cannot be merged here. Keep one of the two.".to_string(),
            ));
        }
        let journal_dir = self.journal_state_dir()?;
        let io = |source| EngineError::Io {
            path: format!("your draft file '{path}' in '{domain}'"),
            source,
        };
        let abs = crate::overlay_files::file(&journal_dir, domain, actor, path).map_err(io)?;
        let lock = self.write_lock(&abs);
        let guard = lock.lock().await;
        let held = crate::overlay_files::held(&journal_dir, domain, actor, path).map_err(io)?;
        if held == crate::overlay_files::Held::Nothing {
            return Err(EngineError::NotFound(format!(
                "you are not drafting the file '{path}' in domain '{domain}'. This domain \
                 reviews changes, so what a resolution settles is your own draft"
            )));
        }
        let mut kept_file: Option<String> = None;
        let kept = match resolution {
            ops::Resolution::Mine => {
                // The start is the team's file where the gate reads it, the
                // key that differs only by case on a disk that may fold, so
                // Keep mine always clears the conflict. With no snapshot nothing
                // can say where a file starts, and no conflict stands either,
                // so nothing is written.
                if let Some(draft) = self.file_draft_of(domain, &journal_dir, actor, path)? {
                    let start = crate::draft_start::DraftFileStart {
                        path: path.to_string(),
                        start_sha: draft.team,
                        shared: None,
                    };
                    crate::draft_start::save_file(&journal_dir, domain, actor, &start)
                        .map_err(io)?;
                }
                "mine"
            }
            _ => {
                // A file from before the record did not cause its conflict, and
                // its bytes exist nowhere else: they are kept first, as an old
                // page's text is, and a copy that cannot be kept refuses.
                if held == crate::overlay_files::Held::Bytes
                    && self.file_step_of(domain, &journal_dir, actor, path)?
                        == super::draft_conflicts::FileStep::Conflict(
                            DraftConflictKind::UnknownStart,
                        )
                {
                    let bytes = crate::overlay_files::read(&journal_dir, domain, actor, path)
                        .map_err(io)?
                        .unwrap_or_default();
                    let what = format!("a kept copy of your draft file '{path}'");
                    let root = self
                        .kept_dir()
                        .ok_or_else(|| EngineError::Internal(format!("{what}: no state folder")))?;
                    let file = crystalline_core::kept::write_bytes(
                        &root,
                        domain,
                        Some(actor),
                        crystalline_core::kept::KeptKind::Engram,
                        path,
                        chrono::Utc::now(),
                        &bytes,
                    )
                    .map_err(|source| EngineError::Io { path: what, source })?;
                    kept_file = Some(file.display().to_string());
                }
                crate::overlay_files::clear(&journal_dir, domain, actor, path).map_err(io)?;
                crate::maintenance::record_pending(domain);
                "theirs"
            }
        };
        drop(guard);
        self.settle_convergence(domain, actor, path);
        let remaining = match self.draft_conflicts(domain, actor).await {
            Ok(left) => json!(left.len()),
            Err(e) => {
                tracing::warn!(
                    domain,
                    actor,
                    "the conflicts left after settling '{path}' could not be counted: {e}"
                );
                Value::Null
            }
        };
        Ok(json!({
            "resolved": path,
            "remaining": remaining,
            "draft": true,
            "file": true,
            "kept": kept,
            "kept_copy": kept_file.is_some(),
            "kept_file": kept_file,
        }))
    }

    /// What `actor`'s draft file at `path` is now, by the table the gate reads.
    fn file_step_of(
        &self,
        domain: &str,
        journal_dir: &Path,
        actor: &str,
        path: &str,
    ) -> Result<super::draft_conflicts::FileStep> {
        let Some(draft) = self.file_draft_of(domain, journal_dir, actor, path)? else {
            return Ok(super::draft_conflicts::FileStep::InStep);
        };
        let journal = journal_dir.to_path_buf();
        super::draft_conflicts::file_step(&draft, || {
            Ok(
                crate::overlay_journal::journal_record_strict(&journal, domain)
                    .map_err(|source| EngineError::Io {
                        path: format!("the convergence record of '{domain}'"),
                        source,
                    })?
                    .conflicts
                    .get(actor)
                    .is_some_and(|paths| paths.iter().any(|held| held == path)),
            )
        })
    }

    /// `actor`'s draft file at `path` read as the gate reads it, or `None` with
    /// no base snapshot.
    fn file_draft_of(
        &self,
        domain: &str,
        journal_dir: &Path,
        actor: &str,
        path: &str,
    ) -> Result<Option<super::draft_conflicts::FileDraft>> {
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let Some(state) = crystalline_remote::state::OriginState::load(&state_dir)? else {
            return Ok(None);
        };
        let tombstone =
            crate::overlay_files::held(journal_dir, domain, actor, path).map_err(|source| {
                EngineError::Io {
                    path: format!("your draft file '{path}' in '{domain}'"),
                    source,
                }
            })? == crate::overlay_files::Held::Tombstone;
        let entry = crate::overlay_files::FileEntry {
            actor: actor.to_string(),
            path: path.to_string(),
            tombstone,
        };
        let folds = self.file_may_fold_case(domain, &root, path);
        super::draft_conflicts::FileDraft::read(journal_dir, domain, actor, &entry, &state, folds)
            .map(Some)
    }

    /// Whether the domain's own origin state records a conflict at `path`: one
    /// the pull put in the folder every actor shares, which no draft of
    /// anybody's is or ever was.
    fn folder_conflict_at(&self, state_dir: &Path, path: &str) -> Result<bool> {
        Ok(crystalline_remote::state::OriginState::load(state_dir)?
            .is_some_and(|state| state.conflicts.iter().any(|c| c.path == path)))
    }

    /// Settles one actor's draft inside their own overlay, from either end of a
    /// move: a path that is the deletion half of a move settles as its draft,
    /// and the draft takes that deletion along.
    ///
    /// - **Keep mine** leaves the row and moves its start to the team's text,
    ///   so her text replaces the team's when she shares. The start is read
    ///   where the decision reads the team's text: her own path when the team
    ///   holds a file there she did not start from, otherwise where she started,
    ///   which for a case move is the snapshot's real key. A team file that is
    ///   gone leaves no start, so the draft lands as a new file. A draft from
    ///   before the record gets its first one here. Refused when another team
    ///   page answers to the draft's address, since Keep mine cannot clear it.
    /// - **Take theirs** ends the draft, both halves of a move, the deletion
    ///   first. Refused while a room is open over the draft, whose next save
    ///   would write it straight back. A draft from before the record is kept
    ///   as a copy first, and a copy that cannot be kept refuses, since it is
    ///   the last one.
    /// - **Merged** writes the text as her draft, row first, then moves the
    ///   start as Keep mine does. A room open over it meets the new row at its
    ///   next save, as an external change, and merges it.
    ///
    /// Every start saved here clears `shared` and `merging`: a merge mark or a
    /// merged proposal no pass settled must not outlive her choice, nor move
    /// her start back to what the proposal held. Nothing here runs the pass or
    /// settles a proposal: the snapshot is read as it stands, so a merge no
    /// pull has brought in is never read as the team's text.
    ///
    /// Everything is read outside the locks. Each row is then written under its
    /// own draft lock, one at a time, and only while it and its record are
    /// still what was read. The domain's own conflict records are untouched:
    /// those are [`ops::resolve`]'s, they belong to the folder every actor
    /// shares.
    ///
    /// **No conflict is required.** Any path this actor is drafting can be
    /// settled: the arms are what a draft's author may already do to their own
    /// draft, and requiring a conflict would leave one that cannot be read
    /// with no way out.
    async fn resolve_in_overlay(
        &self,
        domain: &str,
        domain_id: DomainId,
        actor: &str,
        path: &str,
        resolution: ops::Resolution<'_>,
        state_dir: &Path,
    ) -> Result<Value> {
        let (_, root, _) = self.origin_spec_for_domain(domain)?;
        let view = DomainView::for_actor(self, domain, &HashSet::new(), actor)?;
        // The kind as she sees it: an old move lists once, at its draft.
        let listed = self.draft_conflicts(domain, actor).await?;
        // With no snapshot nothing can say where a draft starts, so no start
        // is written, as no new draft gets one then.
        let snapshot = crystalline_remote::state::OriginState::load(state_dir)?;
        let can_record = snapshot.is_some();
        let state = snapshot.unwrap_or_else(|| crystalline_remote::state::OriginState::new("", ""));
        let rows = {
            let store = self.store.lock().await;
            store.overlay_entries(domain_id, actor).await?
        };
        let mut reading = self
            .read_drafts(domain, domain_id, actor, rows, &root, state_dir, &state)
            .await?;
        let at = reading
            .rows
            .iter()
            .position(|row| row.path == path)
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "you are not drafting '{path}' in domain '{domain}' any more"
                ))
            })?;
        let (d, half) = if reading.rows[at].tombstone {
            let draft = (0..reading.rows.len())
                .find(|&d| !reading.rows[d].tombstone && reading.half_of(d) == Some(at));
            match draft {
                Some(d) => (d, Some(at)),
                None => (at, None),
            }
        } else {
            (at, reading.half_of(at))
        };
        let row = reading.rows[d].clone();
        let kind = listed
            .iter()
            .find(|conflict| conflict.path == row.path)
            .map(|conflict| conflict.kind);
        let mut kept_copy = false;
        let mut warning: Option<String> = None;
        let kept = match resolution {
            ops::Resolution::Mine => {
                if !row.tombstone {
                    // What the list says, and what the team's copies say, since
                    // the index can read an address the copies do not hold.
                    let listed_other = listed
                        .iter()
                        .find(|conflict| {
                            conflict.path == row.path
                                && conflict.kind == DraftConflictKind::AddressTaken
                        })
                        .and_then(|conflict| conflict.other.clone());
                    let other = match listed_other {
                        Some(other) => Some(other),
                        None => reading.team_file_at_address(&row.permalink)?,
                    };
                    if let Some(other) = other {
                        return Err(EngineError::Conflict(format!(
                            "The team's file {other} now uses the same address as your draft \
                             of {}, so keeping yours would put two pages at one address. Change \
                             the address in Edit merged, or take theirs.",
                            row.path
                        )));
                    }
                }
                let (start, half_start) = settled_starts(&mut reading, d, half, can_record)?;
                self.settle_row(
                    &view,
                    domain,
                    domain_id,
                    actor,
                    &reading,
                    d,
                    Settle::Keep,
                    start,
                )
                .await?;
                if let Some(t) = half {
                    self.settle_half(&view, domain, domain_id, actor, &reading, d, t, half_start)
                        .await?;
                }
                "mine"
            }
            ops::Resolution::Theirs => {
                let open = match self.collab_rooms() {
                    Some(rooms) if !row.tombstone => {
                        rooms
                            .has_live_room(domain, &row.permalink, Some(actor))
                            .await
                    }
                    _ => false,
                };
                if open {
                    return Err(EngineError::Conflict(format!(
                        "'{}' is open in an editor. Close it, then take the team's text.",
                        row.path
                    )));
                }
                // Option A (owner question 2): a draft from before the record
                // did not cause its conflict, and its text exists nowhere else.
                if kind == Some(DraftConflictKind::UnknownStart) && !row.tombstone {
                    self.keep_a_copy(domain, actor, &row)?;
                    kept_copy = true;
                }
                if let Some(t) = half {
                    self.settle_row(
                        &view,
                        domain,
                        domain_id,
                        actor,
                        &reading,
                        t,
                        Settle::Drop,
                        None,
                    )
                    .await?;
                }
                let dropped = self
                    .settle_row(
                        &view,
                        domain,
                        domain_id,
                        actor,
                        &reading,
                        d,
                        Settle::Drop,
                        None,
                    )
                    .await;
                if let Err(e) = dropped {
                    // The draft stands, so its deletion half comes back with it.
                    if let Some(t) = half {
                        self.put_half_back(domain, domain_id, actor, &reading, t)
                            .await;
                    }
                    return Err(e);
                }
                "theirs"
            }
            ops::Resolution::Merged(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    EngineError::Invalid(
                        "the merged content is not valid UTF-8, so it is not an engram".to_string(),
                    )
                })?;
                let address = Engine::overlay_record(&row.path, text)?.permalink;
                // A room is keyed by its draft's address, so a merged text that
                // changes it is refused while a room is open, as the pass's merge
                // is. Asked before any draft lock.
                if !row.tombstone
                    && address != row.permalink
                    && let Some(rooms) = self.collab_rooms()
                    && rooms
                        .has_live_room(domain, &row.permalink, Some(actor))
                        .await
                {
                    return Err(EngineError::Conflict(format!(
                        "'{}' is open in an editor, and the merged text gives it a new address. \
                         Close it, then save the merged text.",
                        row.path
                    )));
                }
                if let Some(other) = reading.team_file_at_address(&address)? {
                    return Err(EngineError::Conflict(format!(
                        "The team's file {other} now uses the same address as your merged text \
                         of {}. Change its address, or take theirs.",
                        row.path
                    )));
                }
                let (start, half_start) = settled_starts(&mut reading, d, half, can_record)?;
                warning = self
                    .settle_row(
                        &view,
                        domain,
                        domain_id,
                        actor,
                        &reading,
                        d,
                        Settle::Write(text),
                        start,
                    )
                    .await?;
                if let Some(t) = half {
                    self.settle_half(&view, domain, domain_id, actor, &reading, d, t, half_start)
                        .await?;
                }
                "merged"
            }
        };
        // The frozen list of drafts from before the record forgets both paths.
        self.settle_convergence(domain, actor, &row.path);
        if let Some(t) = half {
            self.settle_convergence(domain, actor, &reading.rows[t].path);
        }
        let remaining = match self.draft_conflicts(domain, actor).await {
            Ok(conflicts) => json!(conflicts.len()),
            Err(e) => {
                tracing::warn!(
                    domain,
                    actor,
                    "the conflicts left after settling '{}' could not be counted: {e}",
                    row.path
                );
                Value::Null
            }
        };
        let mut receipt = json!({
            "resolved": row.path,
            // `null` when the count could not be read.
            "remaining": remaining,
            // The receipt says where the resolution landed: in this actor's
            // draft, not in the folder the team reviewed.
            "draft": true,
            "kept": kept,
            "kept_copy": kept_copy,
        });
        // What the draft writer says beside a write that landed, under the key
        // every draft write tells its caller with.
        note_unmirrored(&mut receipt, warning);
        Ok(receipt)
    }

    /// [`Engine::settle_row`] for the deletion half `t` of draft `d`'s move,
    /// after the draft settled. A half that changed meanwhile is refused with
    /// a sentence that says the draft is settled and the half is not, so she
    /// knows to settle it again; nothing of hers is lost either way.
    #[allow(clippy::too_many_arguments)]
    async fn settle_half(
        &self,
        view: &DomainView<'_>,
        domain: &str,
        domain_id: DomainId,
        actor: &str,
        reading: &draft_conflicts::Reading<'_>,
        d: usize,
        t: usize,
        start: Option<crate::draft_start::DraftStart>,
    ) -> Result<()> {
        let settled = if self.take_failed_half_settle() {
            Err(EngineError::Internal(
                "the armed failure of a test".to_string(),
            ))
        } else {
            self.settle_row(
                view,
                domain,
                domain_id,
                actor,
                reading,
                t,
                Settle::Keep,
                start,
            )
            .await
            .map(|_| ())
        };
        settled.map_err(|e| {
            tracing::warn!(
                domain,
                actor,
                path = reading.rows[t].path.as_str(),
                "the deletion half of a settled move was not settled: {e}"
            );
            EngineError::Conflict(format!(
                "Your draft of {} is settled, but {}, where you moved it from, changed while it \
                 was being settled and is not. Settle {} again.",
                reading.rows[d].path, reading.rows[t].path, reading.rows[t].path
            ))
        })
    }

    /// Row `i` of `reading`, under its draft lock and only while it and its
    /// record are still what was read: `settle` it, then save `start` when
    /// there is one. A row or record that changed meanwhile is refused, with
    /// nothing written. Answers the draft writer's warning, if it gave one.
    ///
    /// A write takes the pass's three steps (`Engine::take_merge`): the record
    /// is marked `merging`, the row is written, and the start moves with the
    /// mark cleared. A failure after the row leaves the mark, so the next pass
    /// merges again rather than reading the row as her case move landed. A row
    /// that could not be written takes the mark back off.
    #[allow(clippy::too_many_arguments)]
    async fn settle_row(
        &self,
        view: &DomainView<'_>,
        domain: &str,
        domain_id: DomainId,
        actor: &str,
        reading: &draft_conflicts::Reading<'_>,
        i: usize,
        settle: Settle<'_>,
        start: Option<crate::draft_start::DraftStart>,
    ) -> Result<Option<String>> {
        let row = &reading.rows[i];
        let lock = self.draft_lock(domain, actor, &row.path)?;
        let _guard = lock.lock().await;
        if !self
            .still_as_read(view, domain_id, actor, row, reading.records[i].as_ref())
            .await?
        {
            return Err(EngineError::Conflict(format!(
                "your draft of '{}' changed while it was being settled. Look at it again, \
                 then settle it.",
                row.path
            )));
        }
        let mut warning = None;
        let mut wrote = false;
        match settle {
            Settle::Keep => {}
            Settle::Write(text) => {
                // Her own choice, so the replay of a mark left behind tells her
                // of no merge.
                let read = reading.records[i].as_ref().filter(|_| start.is_some());
                if let Some(read) = read {
                    let marked = crate::draft_start::DraftStart {
                        merging: Some(false),
                        ..read.clone()
                    };
                    self.save_start(domain, actor, &marked)?;
                }
                match view.write(domain_id, &row.path, text).await {
                    Ok(said) => warning = said,
                    Err(e) => {
                        if let Some(read) = read
                            && let Err(undo) = self.save_start(domain, actor, read)
                        {
                            tracing::warn!(
                                domain,
                                actor,
                                path = row.path.as_str(),
                                "the merge mark could not be taken off a draft whose merged text \
                                 was not written; the next pass merges it again: {undo}"
                            );
                        }
                        return Err(e);
                    }
                }
                wrote = true;
            }
            Settle::Drop => view.drop(domain_id, &row.path).await?,
        }
        if let Some(start) = start {
            if wrote && self.take_failed_merge_start() {
                return Err(EngineError::Internal(
                    "the armed failure of a test".to_string(),
                ));
            }
            self.save_start(domain, actor, &start)?;
        }
        Ok(warning)
    }

    /// Keep a copy of `row`'s text for its owner, the way a room keeps a text
    /// it could not save. A copy that cannot be kept is an error.
    fn keep_a_copy(&self, domain: &str, actor: &str, row: &StoredEngram) -> Result<()> {
        let what = format!("a kept copy of your draft of '{}'", row.path);
        let kept = self
            .kept_dir()
            .ok_or_else(|| EngineError::Internal(format!("{what}: no state folder")))?;
        crystalline_core::kept::write(
            &kept,
            domain,
            Some(actor),
            crystalline_core::kept::KeptKind::Engram,
            &row.path,
            chrono::Utc::now(),
            &row.content,
        )
        .map(|_| ())
        .map_err(|source| EngineError::Io { path: what, source })
    }

    /// Resolves a single domain's `OriginSpec`, root and state directory for
    /// `origin_share`, `origin_withdraw` and `origin_resolve`: each a
    /// single-domain operation unlike `origin_update`/`origin_status`'s
    /// optional "every domain" mode. Errors with `UnknownDomain` when
    /// unregistered, and with the same "has no origin" message
    /// `origin_spec_for` raises when registered but not origin-connected.
    pub(super) fn origin_spec_for_domain(
        &self,
        domain: &str,
    ) -> Result<(OriginSpec, PathBuf, PathBuf)> {
        let entry = self.domain_entry(domain)?;
        self.origin_spec_for(domain, &entry)
    }

    /// The domains `origin_update`/`origin_status` operate on: the one named
    /// (erroring if it is not registered or has no origin) or every
    /// registered domain with an origin, mirroring `sync_targets`'s
    /// config-then-discovered layering.
    ///
    /// `hidden` is the caller's own set of domains it may not see, and it binds
    /// both arms. A named one is refused exactly as an unregistered one, with
    /// the registered list in the error filtered to what this caller may see.
    /// The unnamed arm - "every domain with an origin" - drops them, which is
    /// the whole of what makes the aggregate form safe: without it a stranger
    /// asking for the standing of "every shared domain" is handed a private
    /// team domain's name, its open proposals and its conflicts, and
    /// `origin_update` additionally pulls into it. Every machine-owner caller
    /// (the CLI, the control socket, the poller, the status block) passes an
    /// empty set, which is the answer they would resolve to anyway.
    fn origin_targets(
        &self,
        domain: Option<&str>,
        hidden: &HashSet<String>,
    ) -> Result<Vec<(String, DomainEntry)>> {
        match domain {
            Some(name) => {
                let entry = self.domain_entry_scoped(name, hidden)?;
                if entry.origin.is_none() {
                    return Err(EngineError::Invalid(format!(
                        "domain '{name}' has no origin; connect it with `crystalline domain add --origin`"
                    )));
                }
                Ok(vec![(name.to_string(), entry)])
            }
            None => {
                let mut out: Vec<(String, DomainEntry)> = Vec::new();
                let config = self.config.read().unwrap();
                for (name, entry) in &config.domains {
                    if entry.origin.is_some() && !hidden.contains(name) {
                        out.push((name.clone(), entry.clone()));
                    }
                }
                for (name, entry) in self.discovered_domains.read().unwrap().iter() {
                    if config.domains.contains_key(name) {
                        continue;
                    }
                    if entry.origin.is_some() && !hidden.contains(name) {
                        out.push((name.clone(), entry.clone()));
                    }
                }
                Ok(out)
            }
        }
    }

    /// The `OriginSpec`, domain root and origin state directory for a
    /// registered domain's origin.
    fn origin_spec_for(
        &self,
        name: &str,
        entry: &DomainEntry,
    ) -> Result<(OriginSpec, PathBuf, PathBuf)> {
        let origin_cfg = entry
            .origin
            .as_ref()
            .ok_or_else(|| EngineError::Invalid(format!("domain '{name}' has no origin")))?;
        let root = entry.file_path().ok_or_else(|| {
            EngineError::Invalid(format!(
                "domain '{name}' has no filesystem root to sync an origin into"
            ))
        })?;
        let state_dir = self.origin_state_dir(name)?;
        let spec = OriginSpec {
            repo: origin_cfg.repo.clone(),
            subpath: origin_cfg.path.clone(),
            branch: origin_cfg.branch().to_string(),
        };
        Ok((spec, root, state_dir))
    }

    /// [`Engine::origin_lock`] for the single-domain operations whose next step
    /// requires a registered domain: the name is checked first, so a lock entry
    /// is never created for a name that is about to fail anyway. The error is
    /// exactly the `UnknownDomain` [`Engine::origin_spec_for_domain`] would
    /// raise a line later, so a registered name behaves identically and an
    /// unregistered one answers the same, only without leaving an entry behind.
    ///
    /// A local copy a connected server hides answers as unregistered here
    /// too, so no share, withdraw, discard or resolve acts on it from any
    /// surface while it is hidden (decision D19).
    pub(super) fn origin_lock_registered(
        &self,
        domain: &str,
    ) -> Result<Arc<tokio::sync::Mutex<()>>> {
        self.domain_entry(domain)?;
        self.refuse_shadowed(domain)?;
        Ok(self.origin_lock(domain))
    }

    /// The per-domain lock serializing origin operations for one domain
    /// name, created lazily on first use. Callers that already hold a
    /// `DomainEntry`, and `origin_add` (whose domain may not be registered
    /// yet), use this directly; every other single-domain caller goes through
    /// [`Engine::origin_lock_registered`].
    pub(super) fn origin_lock(&self, domain: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.origin_locks.lock().unwrap();
        locks
            .entry(domain.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The per-file lock every content write holds across its whole
    /// read-decide-write, created lazily on first use and keyed by the file's
    /// canonical path.
    ///
    /// What it closes is a time-of-check-to-time-of-use race, not a partial
    /// write. Each of the file-domain writes looks at the world and then acts
    /// on what it saw, and between those two steps another writer fits:
    ///
    /// - [`Engine::save_engram`], [`Engine::save_manifest`] and
    ///   [`Engine::delete_engram`] read the file, hash it and compare that
    ///   against the caller's `expected_checksum`. Unlocked, two saves of one
    ///   engram arriving together (two browser tabs, or one tab whose autosave
    ///   overlaps a manual save) both read the same text, both find their token
    ///   fresh and both write. One author's version then wins on disk while the
    ///   other is told the save succeeded, which is precisely the outcome
    ///   `If-Match` exists to prevent.
    /// - [`Engine::write_engram_as`] checks that the permalink is free and then
    ///   creates the file. Two creates of one title would both find it free and
    ///   both write, and the second would answer 201 over the first's body
    ///   rather than the 409 that says it was already taken.
    /// - [`Engine::edit_engram_as`] and [`Engine::retire_engram_as`] serialize
    ///   their read-modify-write under the lock: each reads the file, applies
    ///   its operation to that text and writes the result. Two edits, or an
    ///   agent's edit racing a browser's save, would each compute from a
    ///   version the other has already replaced, and the last write would
    ///   silently drop the other's change; under the lock the second one reads
    ///   the first's result and applies to that instead. `edit_engram_as`
    ///   additionally compares an `expected_checksum` there when one is
    ///   supplied (see [`crate::params::EditParams`]), refusing a stale edit
    ///   rather than merely serializing it. A retirement takes its successor's
    ///   lock too, for the reciprocal line it appends there, but never at the
    ///   same time as its target's.
    ///
    /// Held across the whole sequence, the second caller sees the first's bytes
    /// and either refuses or builds on them.
    ///
    /// Keyed by the file's own identity rather than by `domain/permalink`, so
    /// two domains registered over one root, or over two spellings of one path,
    /// still serialize on the file itself: the key is
    /// [`canonicalize`](std::fs::canonicalize)d where the filesystem can
    /// resolve it, which covers symlinks and `..` segments, and for a file that
    /// does not exist yet is its nearest existing ancestor canonicalized with
    /// the rest appended, so the key is the same before the folder exists, after
    /// it does and after the file does (see `lock_key`). Taken before the store lock,
    /// always, so the two never invert; virtual domains take neither, since
    /// their compare-and-swap happens inside a single database statement.
    ///
    /// The map is never pruned, like [`Engine::origin_locks`]: an entry is a
    /// path string and an `Arc`, and the set of files a process ever writes is
    /// bounded by the installation.
    ///
    /// **In-process only.** Two Crystalline processes over one domain root are
    /// not held apart by this; the host-lock machinery governs that.
    pub(crate) fn write_lock(&self, abs: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.keyed_write_lock(abs).1
    }

    /// [`Engine::write_lock`] with the key it answers to. One lookup, so a
    /// caller that orders locks by key orders the very mutexes it will take: a
    /// second lookup can answer differently once another spelling of a missing
    /// file has appeared on a disk that folds case.
    pub(super) fn keyed_write_lock(&self, abs: &Path) -> (String, Arc<tokio::sync::Mutex<()>>) {
        // Computed before the map-wide guard is taken: `canonicalize` is a
        // blocking stat, and every other file's lookup would otherwise queue
        // behind it while it resolves this one's path.
        let key = lock_key(abs);
        let mut locks = self.write_locks.lock().unwrap();
        let lock = locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        (key, lock)
    }

    /// The one lock a draft is written under, for every verb that writes one.
    ///
    /// A draft has no file in the domain's folder - that is the whole of review
    /// mode - so the base file's lock holds nothing apart from it: in a
    /// reviewing domain nobody writes the folder at all, and two writers of one
    /// draft that took it would be two writers holding a mutex neither of them
    /// contends. The draft's own mirror in the overlay journal IS the file this
    /// write produces, so its path is the key, and every arm that writes a
    /// draft takes it: the capture, the edit, the save, the delete and both
    /// halves of a move.
    ///
    /// What the lock is held across is a read-modify-write with no compare
    /// behind it. A capture replaces the whole row; an edit reads the draft,
    /// applies its operation and writes the result; a save compares a checksum
    /// it read a moment ago. Each store write is atomic on its own, which is
    /// exactly why a lost update here is invisible - the capture's receipt says
    /// it landed and the edit that resumed with the older text quietly replaces
    /// it. Serializing them makes the loser read the winner's bytes and either
    /// refuse (a stale checksum, a taken permalink) or build on them.
    ///
    /// Keyed through [`crate::overlay_journal::entry_path`] rather than by
    /// hand, so the lock and the mirror can never come to name two different
    /// places, and taken before the store lock like every other holder. See
    /// [`Engine::write_lock`], whose map this shares: a base file's path and a
    /// draft mirror's path are different keys in one map, so no cycle is formed
    /// and a domain that takes changes directly is untouched by this.
    ///
    /// Keyed on the PATH, so two captures by one actor at two paths that both
    /// claim one permalink are not serialized against each other and both
    /// land; the conflict target is `(domain_id, path, actor)`, so no store
    /// constraint backstops it. The direct-mode file arm has the same hole
    /// under the same key, so this is parity, not a regression (whole-branch
    /// re-review, 2026-09-17).
    pub(super) fn draft_lock(
        &self,
        domain: &str,
        actor: &str,
        path: &str,
    ) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let state_dir = self.journal_state_dir()?;
        let mirror = crate::overlay_journal::entry_path(&state_dir, domain, actor, path)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        Ok(self.write_lock(&mirror))
    }

    /// The base directory per-domain origin state lives under: the test
    /// override, or the real state directory.
    pub(super) fn origins_base_dir(&self) -> Result<PathBuf> {
        match &self.origins_dir_override {
            Some(p) => Ok(p.clone()),
            None => crystalline_core::config::origins_state_dir()
                .map_err(|e| EngineError::Internal(e.to_string())),
        }
    }

    /// One domain's origin state directory (base snapshot, conflict records,
    /// `state.json`).
    ///
    /// `pub(crate)` rather than private: [`crate::nudge::memo_key`] keys its
    /// share-walk memo on this path beside a domain's folder, because two
    /// domain names can register the same folder under different origins
    /// directories, and the memoized answer depends on which one.
    pub(crate) fn origin_state_dir(&self, domain: &str) -> Result<PathBuf> {
        Ok(self.origins_base_dir()?.join(domain))
    }

    /// The state directory the overlay journal lives under: the test override,
    /// or the real one. `<state_dir>/overlays/<domain>/<actor>/<path>` is the
    /// journal's own layout, which [`crate::overlay_journal`] owns.
    /// **This resolver performs no I/O, and that is load bearing.** Its failure
    /// means "this process knows no path at all", a fact about the environment,
    /// which is why [`Engine::overlay_domain_files`] may read it as nothing
    /// there is rather than as something it cannot see: nothing can ever have
    /// been written through a resolver that answers no path. A `create_dir_all`
    /// or a `canonicalize` added here would turn a real permission failure on
    /// the state root into that same answer, and the silent omission that arm
    /// is safe from today would reopen. Every directory failure is detected
    /// strictly after this call, inside `overlay_files::by_actor`.
    pub(crate) fn journal_state_dir(&self) -> Result<PathBuf> {
        match &self.state_dir_override {
            Some(p) => Ok(p.clone()),
            // **Under the test seam this refuses instead of falling back**, and
            // it is the one resolver in this file that does. Every other one
            // reaching a real machine path costs a test a read; this one is
            // reached by `journal_remove_domain`, which is
            // `crystalline_core::tree::remove_tree` under `<state_dir>/overlays/<domain>`.
            // A fixture that forgot [`Engine::with_state_dir`] would delete a
            // developer's own drafts by domain name, silently (the sweep is
            // best effort) and unrecoverably (the journal is the only copy of a
            // draft a rebuild cannot make again) - and would read their real
            // journal into its test index on the way. An audit of the fixtures
            // is not enough; the resolver has to say no.
            #[cfg(any(test, feature = "testing"))]
            None => Err(EngineError::Internal(
                "this engine was built without a state directory, so it reaches no overlay \
                 journal: a test that touches drafts, a removal or a sync must say where the \
                 journal lives with `Engine::with_state_dir`"
                    .to_string(),
            )),
            #[cfg(not(any(test, feature = "testing")))]
            None => crystalline_core::config::state_dir()
                .map_err(|e| EngineError::Internal(e.to_string())),
        }
    }

    /// Resolves the provider an origin operation runs its GitHub calls
    /// through: the injected test provider when one is set, or a fresh
    /// `GitHubProvider` built from the current config and the cached GitHub
    /// token (read from the OS keychain at most once per process, see
    /// [`Engine::github_credential`]). A `connect` earlier this same process
    /// is picked up without a restart - the connect refreshes the cache - and
    /// a machine that has not connected yet is never cached, so a later
    /// connect is seen too. Errors with `RemoteError::NotConnected` when no
    /// token has been saved and no test provider is injected.
    ///
    /// With a GitHub App configured (see [`Engine::github_app`]) the provider
    /// is the App's [`github_app::AppRouter`] instead, which sends each call
    /// on the installation token of the repository it is about, and the
    /// instance credential is not read at all. A repository the App is not
    /// installed on is refused with `RemoteError::AppNotInstalled` naming the
    /// install url, never served on the instance credential. An App that is
    /// configured but unusable refuses every operation the same way.
    pub(super) fn resolve_origin_provider(&self) -> Result<Arc<dyn Provider>> {
        if let Some(p) = &self.origin_provider_override {
            return Ok(p.clone());
        }
        match self.github_app_slot() {
            github_app::AppSlot::Ready(router) => return Ok(router),
            github_app::AppSlot::Refused(detail) => return Err(github_app::app_refusal(detail)),
            github_app::AppSlot::Absent | github_app::AppSlot::Unloaded => {}
        }
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        let (_store, token) = self.github_credential(host.as_deref())?;
        let token = token.ok_or(RemoteError::NotConnected)?;
        Ok(Arc::new(GitHubProvider::new(
            api_url,
            Some(token.access_token),
        )))
    }

    /// Resolves the provider a WRITE verb runs its GitHub calls through, plus
    /// the login it acts as (the acting `StoredToken.user`, for a proposal's
    /// recorded author).
    ///
    /// The read side is [`Engine::resolve_origin_provider`] and never moves.
    /// This one splits by `github.share_identity`, read LIVE on every call
    /// rather than snapshotted at start: the setting is `startup_effective:
    /// false`, so a mode flipped through `configure` is honoured by the very
    /// next share with no restart.
    ///
    /// With the key absent the mode follows the App
    /// ([`Engine::share_identity_effective`]).
    ///
    /// - `instance`: byte for byte what the read side does, the one instance
    ///   credential, whoever the actor is.
    /// - `personal`: the actor's own credential -
    ///   [`ShareActor::Owner`] the fixed `owner` slot, [`ShareActor::Account`]
    ///   that account's, [`ShareActor::HttpAgent`] the account
    ///   `github.agent_identity` names. No personal token on file refuses with
    ///   a teaching text; there is no fallback to the instance credential, by
    ///   design (spec section 6).
    ///
    /// The test provider override short-circuits BOTH modes: an injected mock
    /// has no credential behind it to read a login off, so the login it acts as
    /// is whatever [`Engine::with_origin_provider_login`] supplied beside it -
    /// `None` unless a test asked for one, which is the same answer as a
    /// credential that names nobody.
    ///
    /// Through a GitHub App, the share's commits name the acting account as
    /// author and the App's bot as committer when the account has a contact
    /// address (see [`Engine::app_share_router`]); `repo` is the repository
    /// whose installation token reads the bot's identity.
    pub(super) async fn resolve_share_provider(
        &self,
        actor: &ShareActor,
        repo: &str,
    ) -> Result<(Arc<dyn Provider>, Option<String>)> {
        if self.take_refused_personal_credential() {
            return Err(RemoteError::Refused(PERSONAL_TOKEN_MISSING.to_string()).into());
        }
        if let Some(p) = &self.origin_provider_override {
            return Ok((p.clone(), self.origin_provider_override_login.clone()));
        }
        // Once, here: the steps below must agree, or a personal share could
        // fall back to the instance credential when the mode moves between them.
        let effective = self.share_identity_loaded();
        if let Some(app) = self
            .instance_share_app(effective, actor)
            .map_err(|e| refusal_for(actor, e))?
        {
            let app = self.app_share_router(app, actor, repo).await;
            let login = app.bot_login();
            return Ok((app, login));
        }
        let (api_url, token) = match self
            .personal_app_identity(actor, effective)
            .map_err(|e| refusal_for(actor, e))?
        {
            Some(name) => (
                self.share_api_url(),
                self.personal_user_token(&name)
                    .await
                    .map_err(|e| refusal_for(actor, e))?,
            ),
            None => self
                .resolve_share_credential_in(actor, effective.mode)
                .map_err(|e| refusal_for(actor, e))?,
        };
        let login = token.user_display().map(str::to_string);
        Ok((
            Arc::new(GitHubProvider::new(api_url, Some(token.access_token))),
            login,
        ))
    }

    /// The login a share by `actor` would go out on, the one
    /// [`Engine::resolve_share_provider`] hands the share, without building a
    /// client: `Some(None)` for a credential that names nobody, and `None`
    /// when no credential resolves for `actor` (not connected, no personal
    /// token, no agent identity), which a status reads as "nobody's refusal".
    fn share_login_for(&self, actor: &ShareActor) -> Option<Option<String>> {
        if self.origin_provider_override.is_some() {
            return Some(self.origin_provider_override_login.clone());
        }
        let effective = self.share_identity_loaded();
        match self.instance_share_app(effective, actor) {
            Ok(Some(app)) => return Some(app.bot_login()),
            Ok(None) => {}
            Err(_) => return None,
        }
        if effective.mode == ShareIdentityMode::Personal {
            // A status asks this on every read, so a token that is not there is
            // not looked for again each time.
            let agent = self
                .config
                .read()
                .unwrap()
                .github_agent_identity()
                .map(str::to_string);
            let name = self.acting_identity_name(actor, agent).ok()?;
            let token = self.personal_token_for_report(&name)?;
            return Some(token.user_display().map(str::to_string));
        }
        let (_, token) = self
            .resolve_share_credential_in(actor, effective.mode)
            .ok()?;
        Some(token.user_display().map(str::to_string))
    }

    /// [`Engine::resolve_share_credential_in`] on the mode in effect, for the
    /// tests that exercise its matrix.
    #[cfg(test)]
    pub(super) fn resolve_share_credential(
        &self,
        actor: &ShareActor,
    ) -> Result<(Option<String>, StoredToken)> {
        let mode = self.share_identity_loaded().mode;
        self.resolve_share_credential_in(actor, mode)
    }

    /// The personal identity a share by `actor` runs under when it goes out
    /// on a GitHub App user token: in `personal` mode with a usable App. That
    /// token is renewed before use ([`Engine::personal_user_token`]); without
    /// an App, or with one that cannot be used, the personal credential is
    /// read as before ([`Engine::resolve_share_credential_in`]).
    fn personal_app_identity(
        &self,
        actor: &ShareActor,
        effective: EffectiveShareIdentity,
    ) -> Result<Option<String>> {
        let agent_identity = self
            .config
            .read()
            .unwrap()
            .github_agent_identity()
            .map(str::to_string);
        if effective.mode != ShareIdentityMode::Personal
            || !matches!(self.github_app_slot(), github_app::AppSlot::Ready(_))
        {
            return Ok(None);
        }
        self.acting_identity_name(actor, agent_identity).map(Some)
    }

    /// The effective `github.api_url` a share's client is built against.
    fn share_api_url(&self) -> Option<String> {
        self.config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone())
    }

    /// The App a share goes out through, when it goes out through one: in
    /// `instance` mode with an App configured, the same
    /// [`github_app::AppRouter`] the read side uses. `personal` mode never
    /// takes the App: a person's write runs on that person's own credential.
    ///
    /// An unusable App refuses with its own reason, in `instance` mode and also
    /// when `personal` is only the default: telling a person to link GitHub
    /// through an App that cannot link anybody would send them nowhere. An
    /// admin who set `personal` by name keeps the personal credential.
    ///
    /// A scheduled run with no account ([`ShareActor::Machine`]) is the machine's
    /// own work: it takes the App when `personal` is only the default. An admin
    /// who set `personal` by name gets the owner's identity there too.
    fn instance_share_app(
        &self,
        effective: EffectiveShareIdentity,
        actor: &ShareActor,
    ) -> Result<Option<Arc<github_app::AppRouter>>> {
        let on_the_app = effective.mode == ShareIdentityMode::Instance
            || (*actor == ShareActor::Machine && effective.is_default());
        match self.github_app_slot() {
            github_app::AppSlot::Refused(detail)
                if effective.mode == ShareIdentityMode::Instance || effective.is_default() =>
            {
                Err(github_app::app_refusal(detail))
            }
            github_app::AppSlot::Ready(router) if on_the_app => Ok(Some(router)),
            _ => Ok(None),
        }
    }

    /// The credential half of [`Engine::resolve_share_provider`]: the api url
    /// and the token a write goes out on, before an HTTP client exists.
    ///
    /// Split out because this is where every decision lives - the mode, the
    /// actor mapping, the two refusals - while building the client is
    /// mechanical, and because a `reqwest` client build loads the platform
    /// trust store, which is slow enough to be worth keeping out of the tests
    /// that exercise this matrix.
    fn resolve_share_credential_in(
        &self,
        actor: &ShareActor,
        mode: ShareIdentityMode,
    ) -> Result<(Option<String>, StoredToken)> {
        let (api_url, agent_identity) = {
            let config = self.config.read().unwrap();
            (
                config.github.as_ref().and_then(|g| g.api_url.clone()),
                config.github_agent_identity().map(str::to_string),
            )
        };
        let identity = match mode {
            ShareIdentityMode::Instance => TokenIdentity::Instance,
            ShareIdentityMode::Personal => {
                TokenIdentity::Personal(self.acting_identity_name(actor, agent_identity)?)
            }
        };
        let host = origin::token_host(api_url.as_deref());
        let (_store, token) = self.github_credential_for(&identity, host.as_deref())?;
        let token = match token {
            Some(token) => token,
            // The two absences are different failures and read differently: an
            // instance with no credential at all is simply not connected, while
            // an instance that shares personally and holds no token for THIS
            // identity is connected and still refusing, which is the case that
            // needs teaching.
            None if identity == TokenIdentity::Instance => {
                return Err(RemoteError::NotConnected.into());
            }
            None => return Err(RemoteError::Refused(PERSONAL_TOKEN_MISSING.to_string()).into()),
        };
        Ok((api_url, token))
    }

    /// The login a write failure is enriched in the name of: the acting login
    /// when this instance shares personally, `None` otherwise.
    ///
    /// Instance-token failures keep today's texts (spec section 8), so the
    /// teaching in [`enrich_write_error`] must not fire for them - and the mode
    /// is read live here for the same reason it is read live in
    /// [`Engine::resolve_share_provider`], one call after it, so the two agree
    /// about which credential the call in flight actually used.
    ///
    /// A bot is never a person to teach: a run that went through the App reads
    /// like the instance credential it is.
    fn personal_write_login(&self, login: Option<&str>) -> Option<String> {
        match self.share_identity_loaded().mode {
            ShareIdentityMode::Personal => login
                .filter(|login| !login.ends_with("[bot]"))
                .map(str::to_string),
            ShareIdentityMode::Instance => None,
        }
    }

    /// The personal identity name a write runs under, in personal mode.
    ///
    /// The machine owner has no account to be, so it gets the one fixed local
    /// name; an account is itself, whether it signed in to Fluid or
    /// authenticated an MCP session; an unauthenticated HTTP-MCP agent is
    /// whoever `github.agent_identity` names, or a refusal that says which
    /// setting to write.
    fn acting_identity_name(
        &self,
        actor: &ShareActor,
        agent_identity: Option<String>,
    ) -> Result<String> {
        let name = match actor {
            ShareActor::Owner | ShareActor::Machine => OWNER_IDENTITY_NAME.to_string(),
            ShareActor::Account(name) => name.clone(),
            ShareActor::HttpAgent => agent_identity.ok_or_else(|| {
                EngineError::Remote(RemoteError::Refused(AGENT_IDENTITY_UNSET.to_string()))
            })?,
        };
        // Normalization belongs to the layer that mints these names - the auth
        // store (`crate::rest::auth_store`) trims and lowercases an account name
        // before it is ever stored, and the settings layer holds
        // `github.agent_identity` to the same shape - so the engine asserts the
        // invariant rather than quietly re-normalizing and hiding a surface that
        // stopped honouring it. The token store refuses a malformed name anyway;
        // this is the earlier, louder signal in a debug build.
        debug_assert_eq!(
            name,
            name.trim().to_lowercase(),
            "account names reach the engine already trimmed and lowercased"
        );
        Ok(name)
    }

    /// This machine's GitHub connection, for `origin_status`: `{ connected,
    /// user, token_store }`. With an injected test provider, reflects the
    /// mock's own identity instead of the real token store, so origin tests
    /// never touch the OS keychain or a real credential file. `user` renders
    /// as JSON `null` rather than an empty string for the environment token
    /// store, whose synthesized identity has no login attached (see
    /// `StoredToken::user_display`).
    pub(super) async fn origin_connection_json(&self) -> Result<Value> {
        if let Some(provider) = &self.origin_provider_override {
            let user = provider.current_user().await.ok();
            return Ok(json!({ "connected": true, "user": user, "token_store": "file" }));
        }
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        let (store, token) = self.github_credential(host.as_deref())?;
        // An App is a connection of its own: an instance that runs one
        // reads, pulls and shares without an instance token. Asked only when
        // no token is on file, so an instance without an App reads exactly
        // what it read before.
        let connected =
            token.is_some() || matches!(self.github_app_slot(), github_app::AppSlot::Ready(_));
        Ok(json!({
            "connected": connected,
            "user": token.as_ref().and_then(|t| t.user_display()),
            "token_store": store.kind(),
        }))
    }

    /// How many of one team domain's unshared substantive changes `account`
    /// last wrote, by the changed file's own `generated.by` line.
    ///
    /// `None` for a domain with no origin state to compare against, which is
    /// the same answer a domain with no origin gets: nothing is known to be
    /// unshared, so nothing is known to be anybody's. Never an error - this
    /// enriches a report, and a report survives an unreadable working tree.
    ///
    /// Last-writer provenance, never authorship: it says which actor wrote the
    /// revision on disk, not who the knowledge belongs to.
    ///
    /// Under the domain's `Engine::origin_lock`, for the reason
    /// `Engine::share_facts` gives: a status read that races a share would
    /// otherwise compare a half-written pair and attribute a delta nobody made.
    pub async fn owned_local_changes(&self, domain: &str, account: &str) -> Option<u64> {
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        let (_spec, root, state_dir) = self.origin_spec_for_domain(domain).ok()?;
        let work = origin::unshared_work(&root, &state_dir)?;
        Some(work.owned_by(&root, &format!("human:{account}")))
    }

    /// [`Engine::origin_connection_json`] plus the two facts a caller needs to
    /// know WHICH credential a share of theirs would go out on: the mode
    /// (`share_identity`, always) and, in personal mode, the machine owner's
    /// own connection (`owner_identity`, absent in instance mode because there
    /// is no personal slot in play).
    ///
    /// Only `origin_status` is enriched, not [`Engine::origin_connection_json`]
    /// itself: the settings screen and the `configure` snapshot report the
    /// instance connection, and the personal identities they care about are the
    /// SESSION's, served by `/me/github-identity`.
    ///
    /// Two slots, because two callers resolve two different credentials.
    /// `owner_identity` is the machine OWNER's, which is what a CLI or
    /// stdio-MCP share resolves ([`Engine::acting_identity_name`]).
    /// `agent_identity` is the one an HTTP-MCP peer's share runs as
    /// (`github.agent_identity`), and it is reported on exactly the same terms:
    /// personal mode only, and only when that setting names an account at all,
    /// because an unset agent slot is not a connection somebody has failed to
    /// make - it is a deployment that has no HTTP agent sharing on it. Both
    /// carry `{ account, connected, user }`, and neither is a claim about who
    /// the reader is: a caller reads the slot it would share on.
    ///
    /// A credential that cannot be resolved reports as not connected rather
    /// than failing the whole status read: this is a report, and every other
    /// line of it survives an unreadable credential store.
    ///
    /// A token that is not there is looked for at most once per
    /// [`TOKEN_MISSING_FOR`] ([`Engine::personal_token_for_report`]), so a
    /// standalone `crystalline connect github --personal` is seen when it tells
    /// the daemon, or at the latest when that memory runs out.
    async fn origin_status_connection(&self) -> Result<Value> {
        let mut connection = self.origin_connection_json().await?;
        let agent = self
            .config
            .read()
            .unwrap()
            .github_agent_identity()
            .map(str::to_string);
        let effective = self.share_identity_loaded();
        let mode = effective.mode;
        connection["share_identity"] = json!(mode.as_str());
        connection["share_identity_default"] = json!(effective.is_default());
        connection["share_identity_reason"] = json!(effective.reason_text());
        // A configured App that cannot be used refuses every origin
        // operation; this is where the status says why.
        if let github_app::AppSlot::Refused(detail) = self.github_app_slot() {
            connection["app"] = json!({
                "error": github_app::app_refusal(detail).to_string(),
            });
        }
        // A keychain that could not be read is said here while the failure
        // is remembered: an App saved in it is refused, and without one the
        // status says the keychain was not readable all the same.
        if let Some(reason) = self.github_app_keychain_unreadable() {
            connection["app"]["keychain_unreadable"] = json!(reason);
        }
        if mode == ShareIdentityMode::Personal {
            connection["owner_identity"] =
                self.personal_slot_json(OWNER_IDENTITY_NAME, &connection);
            // Only where an HTTP agent has a slot at all: an absent
            // `github.agent_identity` means no share ever runs as one, and a
            // slot reported for it would read as a connection somebody forgot
            // to make.
            if let Some(agent) = agent.as_deref() {
                connection["agent_identity"] = self.personal_slot_json(agent, &connection);
            }
        }
        Ok(connection)
    }

    /// One personal identity slot for the status connection block:
    /// `{ account, connected, user }`, read from the credential store for
    /// `account`.
    ///
    /// A credential that cannot be resolved reports as not connected rather
    /// than failing the whole status read.
    ///
    /// With an injected test provider the store is never touched at all -
    /// reading it would reach the machine's real keychain from a test - and the
    /// login is deliberately null rather than the mock's: that login belongs to
    /// the injected INSTANCE provider, and reporting it here would invent a
    /// personal connection nobody made, the one thing this must never do, since
    /// the whole point of the slot is to say whether a share can go out at all.
    fn personal_slot_json(&self, account: &str, connection: &Value) -> Value {
        let (connected, user) = if self.origin_provider_override.is_some() {
            (
                connection["connected"].as_bool().unwrap_or(false),
                Value::Null,
            )
        } else {
            let token = self.personal_token_for_report(account);
            (
                token.is_some(),
                json!(token.as_ref().and_then(|t| t.user_display())),
            )
        };
        json!({
            "account": account,
            "connected": connected,
            "user": user,
        })
    }

    /// One personal token for a status read, which asks on every poll: a token
    /// the store just did not have is remembered as missing for
    /// [`TOKEN_MISSING_FOR`], so a server whose owner never connected does not
    /// read the store each time. A saved token is in the cache and wins at once,
    /// and the CLI tells a running daemon about a token it wrote
    /// (`forget_cached_credential`). The write paths never read this memory.
    pub(super) fn personal_token_for_report(&self, account: &str) -> Option<StoredToken> {
        let identity = TokenIdentity::Personal(account.to_string());
        let host = self.github_token_host();
        let key = credential_cache_key(&identity, host.as_deref());
        if let Some(cached) = self.github_tokens.lock().unwrap().get(&key) {
            return Some(cached.token.clone());
        }
        let now = (self.app_clock)();
        let remembered = self
            .github_tokens_missing
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|at| now.saturating_duration_since(*at) < TOKEN_MISSING_FOR);
        if remembered {
            return None;
        }
        // A store that cannot be read is not remembered: it may answer next.
        let (_store, token) = self
            .github_credential_for(&identity, host.as_deref())
            .ok()?;
        let mut missing = self.github_tokens_missing.lock().unwrap();
        if token.is_none() {
            missing.insert(key, now);
        } else {
            missing.remove(&key);
        }
        token
    }

    /// Forgets every remembered missing token whose key starts with `prefix`.
    pub(super) fn forget_token_missing(&self, prefix: &str) {
        self.github_tokens_missing
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(prefix));
    }

    /// The INSTANCE GitHub token store for `host` and the token it holds -
    /// [`Engine::github_credential_for`] with the instance identity - reading
    /// the OS keychain at most once per process. Every read verb and every
    /// connection-status surface goes through here; only a write in personal
    /// mode addresses another identity. The environment token wins first
    /// (`CRYSTALLINE_GITHUB_TOKEN`, via `self.overlay`; keyring-free and never
    /// cached, so unsetting it is picked up live); then a cached present-token
    /// for this host; then the resolved store - the test file override (see
    /// [`Engine::with_token_store_dir`], a plain file that never touches the
    /// real OS keychain), or the real `TokenStore::resolve_and_load`, whose
    /// single `get_password` both picks the backend and loads the token. Only
    /// a present token is cached: a `None` stays live so a later `connect`
    /// (here, or from a standalone CLI writing the same keychain item) is seen
    /// on the next call without a restart. Replaces the old per-operation
    /// resolve-then-load double read that turned every origin op into two
    /// keychain touches.
    ///
    /// The environment wins over the test override too, so a poller or connect
    /// test can prove the env token is actually what gets used even when a
    /// token directory is also wired up.
    pub(super) fn github_credential(
        &self,
        host: Option<&str>,
    ) -> Result<(TokenStore, Option<StoredToken>)> {
        self.github_credential_for(&TokenIdentity::Instance, host)
    }

    /// [`Engine::github_credential`] for any identity: the instance credential
    /// or one person's personal one, cached per identity and host so two
    /// identities can never be served the same client.
    ///
    /// The environment token is instance-only and stays that way. One process
    /// serves everybody who reaches it, so a single `CRYSTALLINE_GITHUB_TOKEN`
    /// cannot mean alice's token for one request and bob's for the next; a
    /// personal identity resolves through the keyring or the file store, even on
    /// a machine that sets the variable.
    pub(super) fn github_credential_for(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<(TokenStore, Option<StoredToken>)> {
        if *identity == TokenIdentity::Instance
            && let Some(token) = self.overlay.github_token()
        {
            let store = TokenStore::env(token, host);
            let stored = store.load()?;
            return Ok((store, stored));
        }
        let key = credential_cache_key(identity, host);
        // The std mutex is held across the keychain read on a cache miss on
        // purpose: the critical section never awaits, and single-flighting the
        // first touch under the lock collapses N concurrent first reads (a
        // daemon resolving several team domains at once) into a single keychain
        // prompt instead of a race of N. Every later call is a cache hit and
        // never reaches the read.
        let mut cache = self.github_tokens.lock().unwrap();
        if let Some(cached) = cache.get(&key) {
            return Ok((cached.store.clone(), Some(cached.token.clone())));
        }
        let (store, token) = match &self.token_store_dir_override {
            Some(dir) => {
                let store = TokenStore::file_fallback_for(identity, dir)?;
                let token = store.load()?;
                (store, token)
            }
            None => {
                let base = self.origins_base_dir()?;
                TokenStore::resolve_and_load_for(identity, host, &base)?
            }
        };
        if let Some(token) = &token {
            cache.insert(
                key,
                CachedGithub {
                    store: store.clone(),
                    token: token.clone(),
                },
            );
        }
        Ok((store, token))
    }

    /// The plan a connect flow saves the INSTANCE credential through.
    pub(super) fn github_save_plan(&self, host: Option<&str>) -> Result<TokenSavePlan> {
        self.token_save_plan(&TokenIdentity::Instance, host)
    }

    /// The plan a person's credential is saved through: the same, and bound to
    /// the account the name holds right now, so the write is refused if the name
    /// holds another by the time it lands. Asked when the work begins, not when
    /// it ends. A read that fails refuses here rather than guessing.
    pub(super) async fn github_save_plan_for(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<TokenSavePlan> {
        let mut plan = self.token_save_plan(identity, host)?;
        let TokenIdentity::Personal(name) = identity else {
            return Ok(plan);
        };
        let Some(access) = self.domain_access.get() else {
            return Ok(plan);
        };
        let agent = self
            .config
            .read()
            .unwrap()
            .github_agent_identity()
            .map(str::to_string);
        let born = access
            .account_born(name)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        plan.owner = Some(TokenOwner {
            access: access.clone(),
            name: name.clone(),
            born,
            no_account_is_right: name == OWNER_IDENTITY_NAME
                || agent.as_deref() == Some(name.as_str()),
        });
        Ok(plan)
    }

    /// The plan a connect flow saves its token through: the test file override
    /// or a real `save_resolving`, plus a handle to the token cache to refresh
    /// after the write. `host` is the token host this connect targets, captured
    /// by value so the device-flow task can own the plan across the spawn.
    ///
    /// `identity` decides both halves of where the token lands - the keyring
    /// account (or the fallback file name) and the cache slot the write
    /// refreshes - so a person's connect can never overwrite the machine's
    /// credential, nor leave the machine's client cached under a name it no
    /// longer belongs to.
    fn token_save_plan(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<TokenSavePlan> {
        let target = match &self.token_store_dir_override {
            // The same derivation `github_credential_for` reads back through,
            // rather than a second spelling of the file name here.
            Some(dir) => SaveTarget::File(
                TokenStore::file_fallback_for(identity, dir).map_err(EngineError::Remote)?,
            ),
            None => SaveTarget::Resolve {
                fallback_dir: self.origins_base_dir()?,
            },
        };
        Ok(TokenSavePlan {
            identity: identity.clone(),
            host: host.map(str::to_string),
            target,
            cache: Arc::clone(&self.github_tokens),
            owner: None,
            gate: self.token_writes.clone(),
        })
    }

    /// Clears the whole GitHub token cache when `e` is
    /// [`RemoteError::AuthExpired`] - the mapped GitHub 401, see
    /// `crystalline_remote::github` - so a token rotated or revoked out from
    /// under a long-running daemon is dropped and the next `github_credential`
    /// re-reads from the keychain or file, picking up a standalone CLI connect
    /// that wrote a fresh token while the daemon ran. Coarse on purpose:
    /// clearing every entry (up to one per identity per host now) avoids
    /// threading the offending host through every provider-op call site and
    /// costs only one extra keychain read per host on the next touch.
    ///
    /// With a GitHub App, a 401 has already been retried once on a renewed
    /// installation token inside [`github_app::AppRouter`]; one that still
    /// reaches here means the installation itself changed, so every
    /// installation and token the App cached is forgotten too and the next
    /// call looks the repository up again (and says so when the App is gone).
    /// Only an App already loaded is touched: this never loads one.
    fn drop_github_credential_on_auth(&self, e: &RemoteError) {
        if matches!(e, RemoteError::AuthExpired) {
            self.github_tokens.lock().unwrap().clear();
            if let Some(router) = self.github_app.read().unwrap().ready() {
                router.app().evict_all();
            }
        }
    }
}

/// What a connect finds holding the name it would register under, once every
/// local refusal is decided ([`Engine::holder_of`]).
enum Holder {
    /// Nothing holds the name.
    Free,
    /// A domain connected exactly the way the request asks: the connect is a
    /// retry, answered with the connected state.
    Connected(DomainEntry),
    /// An origin-less file domain rooted at this path, adoptable in place.
    Adopt(PathBuf, DomainEntry),
}

/// The Reflection mode in effect at the root of a domain being subscribed:
/// the index holds no row of it yet, so the files under `reflections/`
/// decide, the fallback [`crystalline_index::reflection_mode`] takes when
/// nothing is recorded.
fn subscribed_reflection_mode(root: &Path) -> crystalline_core::ReflectionMode {
    crystalline_index::reflection_mode(root, std::iter::empty::<&str>())
}

/// Whose drafts the convergence pass in front of a share or a preview may
/// write when its pull applied nothing. A pull that applied something always
/// converges every person's drafts (see `Engine::overlay_share_tree`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SharePass {
    /// Every person's, as an update does: a share writes to the team.
    Everyone,
    /// Only the acting person's: a preview asks about their own drafts.
    Own,
    /// Nobody's: a preview made on the instance's credential for a caller who
    /// connected no identity of their own only reads.
    Nobody,
}

/// Whether `digest` names this draft's text: as it is stored (LF), or as a share
/// stages it for a wholly CRLF team file. A preview's change list and a
/// proposal both record the staged bytes, so a discard or a withdrawal that
/// sends one of those digests back finds the draft unchanged in either form.
fn is_draft_digest(content: &str, digest: &str) -> bool {
    sha256_hex(content.as_bytes()) == digest
        || sha256_hex(crystalline_core::with_line_endings_of(content, b"\r\n").as_bytes()) == digest
}

/// What an update says when the team merged its proposal while the change was
/// on its way. The merge may have come before the push, which the merge then
/// did not hold, or just after it, which it did; the sentence is true for both.
fn merged_before_the_update(number: u64) -> String {
    format!(
        "The team merged proposal #{number} while this change was on its way. If your change \
         is not in it, it stays in your drafts and you can share it again."
    )
}

/// What settling does to one row.
enum Settle<'t> {
    /// Leave the row as it is: only its start moves.
    Keep,
    /// Write this text as the draft.
    Write(&'t str),
    /// End the row, and its record with it.
    Drop,
}

/// The start a team text gives: the text as LF, or for bytes that are not
/// UTF-8 or a key with no readable copy the digest alone, never both. Nothing
/// for a file the team does not hold.
fn start_of(held: &draft_conflicts::Held) -> (Option<String>, Option<String>) {
    match held {
        draft_conflicts::Held::Absent => (None, None),
        draft_conflicts::Held::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => (Some(crystalline_core::to_lf(text).into_owned()), None),
            Err(_) => (None, Some(sha256_hex(bytes))),
        },
        draft_conflicts::Held::Unreadable(sha256) => (None, Some(sha256.clone())),
    }
}

/// The records a settled draft `d` and its deletion half `half` are saved
/// with: each starts from the team's text as it stands, with nothing shared
/// and no merge marked. `None` for each when no start can be known.
///
/// The draft starts where the decision reads the team's text. With a record:
/// its own path when the team holds a file there she did not start from
/// ([`answers_at_path`](crate::draft_merge::answers_at_path)), otherwise its
/// `from`, which a case move keeps at the snapshot's real key. A draft from
/// before the record starts at its own path when the team holds it, otherwise
/// where its old move pair stood, otherwise at its own path. A key the team
/// does not hold leaves no start and no `from`: the draft is a new file. A
/// tombstone always starts at its own path.
fn settled_starts(
    reading: &mut draft_conflicts::Reading<'_>,
    d: usize,
    half: Option<usize>,
    can_record: bool,
) -> Result<(
    Option<crate::draft_start::DraftStart>,
    Option<crate::draft_start::DraftStart>,
)> {
    if !can_record {
        return Ok((None, None));
    }
    let row = reading.rows[d].clone();
    let record = reading.records[d].clone();
    let from = if row.tombstone {
        Some(row.path.clone())
    } else if let Some(record) = &record {
        let team = reading.team(d)?;
        if crate::draft_merge::answers_at_path(&reading.sides(d, &team)) {
            Some(row.path.clone())
        } else {
            record.from.clone()
        }
    } else if !matches!(reading.team_at(&row.path)?, draft_conflicts::Held::Absent) {
        Some(row.path.clone())
    } else {
        Some(half.map_or_else(|| row.path.clone(), |t| reading.rows[t].path.clone()))
    };
    let held = match &from {
        Some(key) => reading.team_at(key)?,
        None => draft_conflicts::Held::Absent,
    };
    let (start, start_sha256) = start_of(&held);
    let from = if row.tombstone || !matches!(held, draft_conflicts::Held::Absent) {
        from
    } else {
        None
    };
    let settled = crate::draft_start::DraftStart {
        path: row.path.clone(),
        from,
        start,
        start_sha256,
        shared: None,
        merged_at: record.and_then(|record| record.merged_at),
        merging: None,
    };
    let half = match half {
        Some(t) => {
            let path = reading.rows[t].path.clone();
            let (start, start_sha256) = start_of(&reading.team_at(&path)?);
            Some(crate::draft_start::DraftStart {
                path: path.clone(),
                from: Some(path),
                start,
                start_sha256,
                shared: None,
                merged_at: reading.records[t]
                    .as_ref()
                    .and_then(|record| record.merged_at),
                merging: None,
            })
        }
        None => None,
    };
    Ok((Some(settled), half))
}
