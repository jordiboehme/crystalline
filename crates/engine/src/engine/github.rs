use super::*;

impl Engine {
    // --- configure: GitHub connect ------------------------------------------

    /// The api url a connect action uses for this one call: `host`
    /// (formatted as a GitHub Enterprise Server api base) when supplied,
    /// otherwise the durable `github.api_url` setting. `host` never persists;
    /// durable Enterprise Server setup is `set github.api_url`.
    fn connect_api_url(&self, host: Option<&str>) -> Option<String> {
        host.map(|h| format!("https://{h}/api/v3")).or_else(|| {
            self.config
                .read()
                .unwrap()
                .github
                .as_ref()
                .and_then(|g| g.api_url.clone())
        })
    }

    /// The OAuth App client id a connect action authenticates as: the
    /// self-hosted override from `github.oauth_client_id` when set, else the
    /// embedded Crystalline client id.
    fn oauth_client_id(&self) -> String {
        self.config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.oauth_client_id.clone())
            .unwrap_or_else(|| crystalline_remote::GITHUB_CLIENT_ID.to_string())
    }

    /// The pending INSTANCE device flow's display view, `{ pending: true,
    /// user_code, verification_url, expires_in_secs }`, or `None` when no
    /// instance flow is running. A person's sign-in is invisible here: the two
    /// are different credentials and neither surface may report the other's
    /// code.
    fn pending_view(&self) -> Option<Value> {
        self.pending_view_for(&TokenIdentity::Instance)
    }

    /// [`Engine::pending_view`] for one identity.
    ///
    /// `expires_in_secs` is what is LEFT of the code's life, not the flow's
    /// original expiry: a caller that polls sees the number fall, which is
    /// what tells a person (or a model relaying to one) that the flow is
    /// alive rather than wedged. It saturates at 0 rather than going
    /// negative; a code whose clock has run out stays reported until the
    /// background task lands its own expiry error, which is the outcome that
    /// clears the slot.
    pub(super) fn pending_view_for(&self, identity: &TokenIdentity) -> Option<Value> {
        self.pending_connect
            .lock()
            .unwrap()
            .as_ref()
            .filter(|p| p.identity == *identity)
            .map(PendingConnect::view)
    }

    /// Takes the pending INSTANCE flow's outcome if it has landed, clearing
    /// the slot so a later connect starts fresh. Returns `None` both when no
    /// instance flow is pending at all and when one is pending but still
    /// waiting on the user; a caller distinguishes those with
    /// [`Engine::pending_view`].
    fn take_finished_pending(&self) -> Option<std::result::Result<String, RemoteError>> {
        self.take_finished_pending_for(&TokenIdentity::Instance)
    }

    /// [`Engine::take_finished_pending`] for one identity: a landed outcome is
    /// reported to - and cleared by - whoever the flow belonged to, never by
    /// the surface that happens to read first.
    pub(super) fn take_finished_pending_for(
        &self,
        identity: &TokenIdentity,
    ) -> Option<std::result::Result<String, RemoteError>> {
        let mut guard = self.pending_connect.lock().unwrap();
        let landed = guard
            .as_ref()
            .filter(|p| p.identity == *identity)
            .and_then(|p| p.outcome.lock().unwrap().take());
        if landed.is_some() {
            *guard = None;
        }
        landed
    }

    /// The stored guidance for `identity`'s pending flow, read without
    /// taking anything. Called BEFORE [`Engine::take_finished_pending_for`]
    /// on the same identity: that call clears the slot the guidance lives
    /// on, so a caller that wants both the outcome and the guidance it
    /// landed with has to read this one first.
    fn pending_next_steps_for(&self, identity: &TokenIdentity) -> Option<String> {
        self.pending_connect
            .lock()
            .unwrap()
            .as_ref()
            .filter(|p| p.identity == *identity)
            .map(|p| p.next_steps.clone())
    }

    /// Drops a pending flow belonging to `identity`, leaving another
    /// identity's alone. What a connect that settles the same credential by
    /// another route (a pasted token) and a disconnect both do: the flow in
    /// flight is about to be answered by a stale background task, and only for
    /// this one credential.
    fn clear_pending_for(&self, identity: &TokenIdentity) {
        let mut guard = self.pending_connect.lock().unwrap();
        if guard.as_ref().is_some_and(|p| p.identity == *identity) {
            *guard = None;
        }
    }

    /// The `github` block of the `configure` tool's snapshot: `{ connected,
    /// user, token_store, pending_connect }`. A flow still waiting on the
    /// user reports `pending_connect`; one that landed since the last call
    /// is reported here exactly once and the slot is cleared - a successful
    /// sign-in folds into `connected`/`user`, while an expired or declined
    /// one is built from [`Engine::origin_connection_json`] the same way the
    /// success case is, so a re-connect attempt on an instance that already
    /// has a working credential still reports `connected: true` and that
    /// credential's `user`/`token_store` - with `error` and `next_steps` (the
    /// guidance the flow started with, see [`Engine::pending_next_steps_for`])
    /// added beside them, telling the caller to connect again and click
    /// Authorize this time, rather than surfacing a bare error a model has
    /// nothing to act on.
    async fn configure_connection_block(&self) -> Result<Value> {
        // Read before `take_finished_pending` below, which clears the very
        // slot this comes from: an outcome cannot land without a
        // `PendingConnect` first existing for the same identity, so the
        // `unwrap_or_default` a few lines down is unreachable in practice -
        // kept only so a landed outcome can never itself fail this call.
        let landed_guidance = self.pending_next_steps_for(&TokenIdentity::Instance);
        if let Some(outcome) = self.take_finished_pending() {
            return match outcome {
                Ok(_user) => {
                    let mut github = self.origin_connection_json().await?;
                    github["pending_connect"] = Value::Null;
                    Ok(github)
                }
                Err(e) => {
                    let mut github = self.origin_connection_json().await?;
                    github["pending_connect"] = Value::Null;
                    github["error"] = json!(e.to_string());
                    github["next_steps"] = json!(Self::retry_guidance(
                        &e,
                        landed_guidance.as_deref().unwrap_or_default()
                    ));
                    Ok(github)
                }
            };
        }
        if let Some(view) = self.pending_view() {
            return Ok(json!({
                "connected": false,
                "user": Value::Null,
                "token_store": Value::Null,
                "pending_connect": view,
            }));
        }
        let mut github = self.origin_connection_json().await?;
        github["pending_connect"] = Value::Null;
        Ok(github)
    }

    /// What to tell the caller after a device flow lands as a failure: retry
    /// wording that names the reason distinctly for an expired code versus a
    /// declined one where the outcome can tell them apart, falling back to a
    /// generic reason otherwise, followed by `landed_guidance` (the same
    /// confirmation guidance the flow started with, so the authorized-apps
    /// url and the Authorize reminder are never phrased twice). Its only
    /// caller is [`Engine::configure_connection_block`] right above; kept as
    /// an associated function (it needs no `self`) rather than a free one so
    /// it stays beside that caller.
    fn retry_guidance(e: &RemoteError, landed_guidance: &str) -> String {
        let reason = match e {
            RemoteError::AuthExpired => "the code expired before it was authorized",
            // `poll_device_flow_once` (crates/remote/src/github/auth.rs) maps
            // GitHub's `access_denied` to exactly this status and message; a
            // 403 from elsewhere in the same background task
            // (validate_token, an enterprise SAML/token restriction) is a 403
            // too, so the message is matched as well as the status rather
            // than assuming every 403 here is a declined device-flow
            // confirmation.
            RemoteError::Api {
                status: 403,
                message,
            } if message.as_str() == "the sign-in was declined on GitHub" => {
                "the sign-in was declined on GitHub"
            }
            _ => "the sign-in did not complete",
        };
        format!(
            "{reason}. Call configure with connect \"github\" again to start a new sign-in, \
             and this time click Authorize on the page after the code. {landed_guidance}"
        )
    }

    /// The token-store host this connect targets: `github.api_url`'s bare
    /// Enterprise Server host, or `None` for GitHub.com. The same derivation
    /// [`Engine::origin_connection_json`] uses, so status, readiness and
    /// disconnect can never look at a different credential slot than
    /// team-domain operations do - on a GitHub Enterprise instance the GHES
    /// token is the one read (and deleted), never an empty github.com slot.
    pub(super) fn github_token_host(&self) -> Option<String> {
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        origin::token_host(api_url.as_deref())
    }

    /// The connection as a settings surface polls it. Mirrors
    /// configure_connection_block's lifecycle handling (the MCP view) so the
    /// two surfaces can never disagree about a pending or finished flow: a
    /// finished failure is reported exactly once via the outcome slot, and a
    /// finished success is simply visible as connected (the token was saved).
    pub async fn github_connection(&self) -> Result<GithubConnection> {
        let error = match self.take_finished_pending() {
            Some(Err(e)) => Some(e.to_string()),
            _ => None,
        };
        let host = self.github_token_host();
        let (store, token) = self.github_credential(host.as_deref())?;
        let pending = self.pending_view().map(|v| GithubPending {
            user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
            verification_url: v["verification_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            expires_in_secs: v["expires_in_secs"].as_u64().unwrap_or_default(),
        });
        Ok(GithubConnection {
            enabled: self.github_enabled(),
            connected: token.is_some(),
            user: token
                .as_ref()
                .and_then(|t| t.user_display())
                .map(str::to_string),
            token_store: token.is_some().then(|| store.kind().to_string()),
            pending,
            error,
        })
    }

    /// Whether team-domain registration can succeed right now.
    pub async fn github_ready(&self) -> bool {
        if !self.github_enabled() {
            return false;
        }
        let host = self.github_token_host();
        matches!(self.github_credential(host.as_deref()), Ok((_, Some(_))))
    }

    /// Forget the stored credential: delete it where it lives, drop its cache
    /// entries and cancel any pending device flow. Refuses under an
    /// environment token (only the environment can retire it) and on a
    /// read-only instance. github.enabled is untouched: turning the feature
    /// off stays a configure concern.
    ///
    /// Only the INSTANCE credential's cache entries go - every host's, since
    /// the delete above may have been for one host while a GHES entry for
    /// another is stale for the same reason. Personal slots stay: they are
    /// different credentials that this call did not delete, and clearing them
    /// would cost every connected person a keychain read (a prompt, on a real
    /// machine) to recover something that never changed. Same rule as
    /// [`Engine::disconnect_github_identity`], read from the other side.
    pub async fn github_disconnect(&self) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let host = self.github_token_host();
        let (store, token) = self.github_credential(host.as_deref())?;
        if matches!(store, TokenStore::Env { .. }) {
            return Err(EngineError::EnvTokenConnect);
        }
        let kind = store.kind();
        if token.is_some() {
            store.delete().map_err(EngineError::Remote)?;
        }
        // Keyed by prefix rather than by the one host this call resolved: the
        // cache holds one entry per identity per host, and the instance's are
        // exactly the ones this delete invalidates.
        let instance_prefix = credential_cache_key(&TokenIdentity::Instance, None);
        self.github_tokens
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(&instance_prefix));
        // Only the machine's own sign-in: a person's device flow is a
        // different credential and is left to run.
        self.clear_pending_for(&TokenIdentity::Instance);
        Ok(json!({ "connected": false, "token_store": kind }))
    }

    /// Wraps a `github` block with the settings registry snapshot, the full
    /// shape the `configure` tool always returns.
    fn configure_snapshot_with(&self, github: Value) -> Result<Value> {
        let file = self.file_config.read().unwrap();
        Ok(json!({ "settings": settings::snapshot(&file, &self.overlay), "github": github }))
    }

    /// The `configure` tool's plain snapshot: every registry setting plus
    /// the GitHub connection block. Used for a bare call and after applying
    /// `set`/`unset`.
    ///
    /// With `github.enabled` off the connection block is `{ github_enabled:
    /// false, note }` and nothing else: no `connected`, no `user`, no
    /// `token_store`, no `pending_connect`. Absent rather than false on
    /// purpose - a `connected: false` would be a claim about a credential
    /// this call deliberately did not read, and reading it is the thing the
    /// gate exists to prevent (on a real machine that read is an OS keychain
    /// touch, for a feature that is switched off). What a disabled instance
    /// reports is the feature's state and how to turn it on, which is the
    /// only actionable thing at that moment: `configure` stays visible with
    /// GitHub off precisely so it can be enabled.
    ///
    /// The gate sits ABOVE [`Engine::configure_connection_block`], whose
    /// first act is draining a landed device-flow outcome. That placement is
    /// deliberate: gating below the drain would leave only two options, both
    /// wrong - report the landed outcome (connection facts on a disabled
    /// instance) or drain and swallow it (destroying the one thing a
    /// report-once contract cannot survive). Above the drain the outcome
    /// stays in the slot and is still reported exactly once, by the next
    /// connect call or by the settings surface through
    /// [`Engine::github_connection`], which drains for itself. The only
    /// change is that a bare `configure` stops being one of the surfaces
    /// that report it while the feature is off.
    ///
    /// The connect paths are NOT gated: [`Engine::connect_with_token`] and
    /// [`Engine::start_device_connect`] build their own block and go through
    /// [`Engine::configure_snapshot_with`], so connecting with
    /// `github.enabled` off still reports the connection and says so in its
    /// note. Connecting and enabling are independent and either order works.
    pub async fn configure_snapshot(&self) -> Result<Value> {
        if !self.github_enabled() {
            return self.configure_snapshot_with(json!({
                "github_enabled": false,
                "note": "GitHub is switched off on this instance; set github.enabled true with configure to connect or read the connection.",
            }));
        }
        let github = self.configure_connection_block().await?;
        self.configure_snapshot_with(github)
    }

    /// The `configure` tool's personal-access-token path: validates `token`
    /// against GitHub (or `host`, for this call only), saves it and reports
    /// the connection. Drops any unrelated pending device flow, since a PAT
    /// connect settles identity immediately and a later-landing background
    /// flow must never overwrite that with a stale report. Refuses up front,
    /// before validating anything against GitHub, when
    /// `CRYSTALLINE_GITHUB_TOKEN` is set: this machine's identity is already
    /// fixed by the environment. The response's `github_enabled` and `note`
    /// state enablement explicitly, straight from the live effective config,
    /// so an agent narrates it from data rather than inferring it from tool
    /// wording (connecting and enabling are independent of each other).
    pub async fn connect_with_token(&self, token: &str, host: Option<&str>) -> Result<Value> {
        if self.overlay.github_token().is_some() {
            return Err(EngineError::EnvTokenConnect);
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let api_url = self.connect_api_url(host);
        let user = self
            .connect_auth
            .validate_token(api_url.as_deref(), token)
            .await?;
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan(token_host.as_deref())?;
        save_off_runtime(
            plan,
            StoredToken {
                access_token: token.to_string(),
                host: token_host.unwrap_or_else(|| "github.com".to_string()),
                user: user.clone(),
                created_at: chrono::Utc::now(),
            },
        )
        .await?;
        self.clear_pending_for(&TokenIdentity::Instance);

        let mut github = self.origin_connection_json().await?;
        github["pending_connect"] = Value::Null;
        let enabled = self.config.read().unwrap().github_enabled();
        github["github_enabled"] = json!(enabled);
        github["note"] = json!(connect_enablement_note(enabled, false));
        self.configure_snapshot_with(github)
    }

    /// The `configure` tool's device-flow path: starts a new sign-in, or
    /// reports the one already running (or just finished), so a second
    /// connect call never starts a second flow. A fresh start spawns a
    /// background task that runs the flow to completion, validates the
    /// token and saves it, stashing the outcome in the pending slot for a
    /// later `configure` call to report and clear (see
    /// [`Engine::configure_connection_block`]). Returns immediately either
    /// way: the caller sees `pending_connect` in the same call that starts
    /// the flow, never blocking on the user confirming the code. Refuses up
    /// front, before starting anything, when `CRYSTALLINE_GITHUB_TOKEN` is
    /// set: this machine's identity is already fixed by the environment.
    ///
    /// `restart` abandons a sign-in already pending and starts a fresh code,
    /// for the person who never saw the first one or let it go stale. Without
    /// it a second call reports the outstanding code as before, now with one
    /// sentence naming `restart` so the way out is in the response rather
    /// than in somebody's memory.
    pub async fn start_device_connect(&self, host: Option<&str>, restart: bool) -> Result<Value> {
        if self.overlay.github_token().is_some() {
            return Err(EngineError::EnvTokenConnect);
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let Some(view) = self
            .begin_device_flow(&TokenIdentity::Instance, host, restart)
            .await?
        else {
            let mut github = self.configure_connection_block().await?;
            // The code is the one already outstanding, so say how to give up
            // on it. Only on this branch: the sentence is about a SECOND
            // connect call, and repeating it on a first one would advertise
            // abandoning a code the caller has not even relayed yet.
            if let Some(next_steps) = github["pending_connect"]["next_steps"].as_str() {
                let extended = format!("{next_steps} {RESTART_SENTENCE}");
                github["pending_connect"]["next_steps"] = json!(extended);
            }
            return self.configure_snapshot_with(github);
        };

        let enabled = self.config.read().unwrap().github_enabled();
        self.configure_snapshot_with(json!({
            "connected": false,
            "user": Value::Null,
            "token_store": Value::Null,
            "pending_connect": view,
            "github_enabled": enabled,
            "note": connect_enablement_note(enabled, true),
        }))
    }

    /// Starts a device-flow sign-in that will store its token as `identity`,
    /// spawning the background task that runs it to completion, validates the
    /// token, saves it and stashes the outcome in the pending slot.
    ///
    /// Answers `Some(view)` with the code to show when a flow was started, and
    /// `None` when this same identity already has one running - the double
    /// click a caller reports the outstanding code for rather than stranding a
    /// second one.
    ///
    /// There is exactly ONE flow slot per engine and it is tagged, so a
    /// sign-in started while a DIFFERENT identity's is still in flight is
    /// refused with [`EngineError::ConnectInProgress`] instead of joining it.
    /// The one exception is a flow whose outcome has already landed: that
    /// sign-in is finished rather than in progress - its token, if any, is
    /// saved and every status reads the store, so the only thing dropped is an
    /// unread error line for a flow nobody came back to look at - and the slot
    /// is taken over.
    ///
    /// `restart` is the escape hatch for the one case that used to have none:
    /// a flow whose code the person lost, or never saw, with a slot that only
    /// ever answered with that same unusable code. With it set, this identity's
    /// pending flow is ABANDONED - its background task aborted and its record
    /// dropped, so the fresh `outcome` slot below cannot be written by the old
    /// task - and a new sign-in is started. It abandons only this identity's
    /// flow: another identity's still refuses with
    /// [`EngineError::ConnectInProgress`], since a restart is a statement
    /// about one's own sign-in, never a licence to cancel somebody else's.
    pub(super) async fn begin_device_flow(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
        restart: bool,
    ) -> Result<Option<Value>> {
        {
            let mut guard = self.pending_connect.lock().unwrap();
            match guard.as_ref() {
                // A landed outcome is not a flow to abandon, so the
                // restart arm asks for one that is still running. Without
                // that, `restart: true` against a sign-in that already
                // finished threw away its one-shot report and answered with a
                // fresh code beside `connected: true`. Falling through to the
                // arm below instead answers `None`, which is what makes the
                // caller's status read drain the outcome and say what
                // happened; the slot is clear afterwards, so a second restart
                // starts fresh.
                Some(p)
                    if p.identity == *identity
                        && restart
                        && p.outcome.lock().unwrap().is_none() =>
                {
                    // Abort first, then drop: the task stops at its next poll
                    // and the record it would have written into is gone
                    // either way, since the fresh flow below builds its own
                    // outcome slot.
                    if let Some(handle) = &p.abort {
                        handle.abort();
                    }
                    tracing::info!(
                        identity = %identity_label(identity),
                        "github device sign-in abandoned on request; starting a fresh code"
                    );
                    *guard = None;
                }
                Some(p) if p.identity == *identity => return Ok(None),
                Some(p) if p.outcome.lock().unwrap().is_some() => *guard = None,
                Some(_) => return Err(EngineError::ConnectInProgress),
                None => {}
            }
        }

        let api_url = self.connect_api_url(host);
        let auth_base = crystalline_remote::github::auth::auth_base(api_url.as_deref());
        let client_id = self.oauth_client_id();
        let label = identity_label(identity);
        // Where this identity's token will be saved, resolved BEFORE anything
        // is started. It is fallible, and it used to run after the pending
        // record was already in the slot, which left a failure holding a slot
        // with no task in it. Resolved here, a failure costs nothing at all:
        // no code has been asked for and no record exists.
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan_for(identity, token_host.as_deref())?;
        let start = match self
            .connect_auth
            .start_device_flow(&auth_base, &client_id)
            .await
        {
            Ok(start) => start,
            Err(e) => {
                tracing::warn!(
                    identity = %label,
                    step = "start",
                    error = %e,
                    "github device sign-in could not be started"
                );
                return Err(e.into());
            }
        };
        tracing::info!(
            identity = %label,
            user_code = %start.user_code,
            expires_in_secs = start.expires_in_secs,
            "github device sign-in started",
        );

        let next_steps = crystalline_remote::github::auth::confirmation_guidance(&auth_base);
        let outcome_slot: Arc<std::sync::Mutex<Option<std::result::Result<String, RemoteError>>>> =
            Arc::new(std::sync::Mutex::new(None));

        let auth = self.connect_auth.clone();
        let task_label = label.clone();
        // Cloned for the record below, which is now built after the spawn: the
        // task owns the poll's copy of the start and the outcome slot.
        let user_code = start.user_code.clone();
        let verification_url = start.verification_url.clone();
        let expires_in_secs = start.expires_in_secs;
        let record_slot = outcome_slot.clone();
        let task = tokio::spawn(async move {
            let result: std::result::Result<String, (&'static str, RemoteError)> = async {
                let access_token = auth
                    .run_device_flow(&auth_base, &client_id, &start)
                    .await
                    .map_err(|e| ("poll", e))?;
                tracing::info!(
                    identity = %task_label,
                    "github device sign-in: access token received from GitHub"
                );
                let user = auth
                    .validate_token(api_url.as_deref(), &access_token)
                    .await
                    .map_err(|e| ("validate", e))?;
                tracing::info!(
                    identity = %task_label,
                    login = %user,
                    "github device sign-in: token validated"
                );
                let stored = StoredToken {
                    access_token,
                    host: token_host
                        .clone()
                        .unwrap_or_else(|| "github.com".to_string()),
                    user: user.clone(),
                    created_at: chrono::Utc::now(),
                };
                // The save touches the OS keychain, which is a blocking call
                // with a bound but no cancellation: off the runtime's worker
                // it goes, so a slow keychain cannot stall unrelated work.
                save_off_runtime(plan, stored)
                    .await
                    .map_err(|e| ("save", e))?;
                Ok(user)
            }
            .await;
            let result = match result {
                Ok(user) => Ok(user),
                Err((step, e)) => {
                    tracing::warn!(
                        identity = %task_label,
                        step,
                        error = %e,
                        "github device sign-in failed"
                    );
                    Err(e)
                }
            };
            *outcome_slot.lock().unwrap() = Some(result);
        });

        // The record is built and inserted AFTER the task, complete, under one
        // lock. It used to be inserted first and have its abort handle written
        // back under a second acquisition, which left a window: two concurrent
        // restarts of the same identity could store the first task's handle on
        // the second record, and a later restart would then abort a task that
        // was already dead while a live one kept polling GitHub. One
        // acquisition, one whole record, no window.
        let pending = PendingConnect {
            identity: identity.clone(),
            user_code,
            verification_url,
            expires_in_secs,
            started_at: tokio::time::Instant::now(),
            next_steps: next_steps.clone(),
            outcome: record_slot,
            abort: Some(task.abort_handle()),
        };
        // The view comes from the record in hand rather than from a read-back
        // of the slot. A `clear_pending_for` landing in that window - a pasted
        // token, a disconnect - made the read-back answer `None`, and a flow
        // that HAD started was then reported as no flow at all.
        let view = pending.view();
        *self.pending_connect.lock().unwrap() = Some(pending);
        Ok(Some(view))
    }

    // --- one account's own GitHub identity ----------------------------------

    /// One account's personal GitHub connection, for the profile card that
    /// manages it: whether a token is on file, the login it was connected as,
    /// since when, where it lives, and the device flow's poll.
    ///
    /// The account name is the identity anchor (spec section 4), so a name a
    /// credential cannot be addressed by is refused here, in words that name
    /// the fix, rather than at the person's first share (see the
    /// `personal_identity` gate every verb on this surface goes through).
    ///
    /// A pure read, like the instance status it mirrors: it is served on a
    /// read-only instance, and it doubles as the device flow's poll, reporting
    /// a failed flow's reason on exactly one read.
    pub async fn github_identity_status(&self, account: &str) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        let error = match self.take_finished_pending_for(&identity) {
            Some(Err(e)) => Some(e.to_string()),
            _ => None,
        };
        let host = self.github_token_host();
        let (store, token) = self.github_credential_for(&identity, host.as_deref())?;
        let pending = self.pending_view_for(&identity).map(|v| GithubPending {
            user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
            verification_url: v["verification_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            expires_in_secs: v["expires_in_secs"].as_u64().unwrap_or_default(),
        });
        Ok(GithubIdentity {
            account: account.to_string(),
            connected: token.is_some(),
            login: token
                .as_ref()
                .and_then(|t| t.user_display())
                .map(str::to_string),
            connected_at: token.as_ref().map(|t| t.created_at),
            token_store: token.is_some().then(|| store.kind().to_string()),
            pending,
            error,
        })
    }

    /// Connect one account's GitHub identity with a personal access token,
    /// validated against GitHub before it is stored so the login on file is
    /// the one the token actually belongs to.
    ///
    /// `CRYSTALLINE_GITHUB_TOKEN` is deliberately NOT a bar here, unlike on
    /// the instance connect: that variable fixes the MACHINE's identity (the
    /// environment store is instance-only, by construction in
    /// `crystalline_remote::token`), and an instance whose machine credential
    /// comes from the environment is exactly the kind that shares personally.
    pub async fn connect_github_identity_token(
        &self,
        account: &str,
        token: &str,
    ) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let api_url = self.connect_api_url(None);
        let user = self
            .connect_auth
            .validate_token(api_url.as_deref(), token)
            .await?;
        let token_host = origin::token_host(api_url.as_deref());
        let plan = self.github_save_plan_for(&identity, token_host.as_deref())?;
        save_off_runtime(
            plan,
            StoredToken {
                access_token: token.to_string(),
                host: token_host.unwrap_or_else(|| "github.com".to_string()),
                user,
                created_at: chrono::Utc::now(),
            },
        )
        .await?;
        // A pasted token settles this identity now, so a device flow of this
        // person's still in flight must not land on top of it later.
        self.clear_pending_for(&identity);
        self.github_identity_status(account).await
    }

    /// Start a device-code sign-in for one account's GitHub identity. Returns
    /// immediately with the status carrying the code to confirm; the flow runs
    /// in the background and its outcome is read from
    /// [`Engine::github_identity_status`].
    ///
    /// One sign-in at a time across the whole engine: a second account's
    /// connect while this one runs is [`EngineError::ConnectInProgress`], and
    /// the same account asking again reports the code already outstanding -
    /// unless `restart` is set, which abandons this account's own pending
    /// flow and issues a fresh code. A restart never touches another
    /// identity's flow; that is still refused.
    pub async fn start_github_identity_device_flow(
        &self,
        account: &str,
        restart: bool,
    ) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.begin_device_flow(&identity, None, restart).await?;
        self.github_identity_status(account).await
    }

    /// Forget one account's GitHub identity: delete the credential where it
    /// lives, drop its cache slot and cancel a device flow of its own.
    /// Idempotent - disconnecting an identity that holds nothing succeeds.
    ///
    /// Only this identity's cache entry is evicted, not the whole cache: every
    /// other credential this process resolved is still valid, and re-reading
    /// them would cost a keychain prompt each for nothing.
    pub async fn disconnect_github_identity(&self, account: &str) -> Result<GithubIdentity> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let host = self.github_token_host();
        let (store, token) = self.github_credential_for(&identity, host.as_deref())?;
        if token.is_some() {
            store.delete().map_err(EngineError::Remote)?;
        }
        self.github_tokens
            .lock()
            .unwrap()
            .remove(&credential_cache_key(&identity, host.as_deref()));
        self.clear_pending_for(&identity);
        self.github_identity_status(account).await
    }

    /// Drop every cached slot for one credential, without touching the
    /// credential itself. `account` is `None` for this machine's own.
    ///
    /// This exists for the credential store's OTHER writer. `crystalline
    /// connect github --disconnect` runs in the CLI process and deletes the
    /// token where it lives, which a running daemon cannot notice: its cache
    /// is a process-lifetime map ([`Engine::github_tokens`]), so it would go
    /// on sharing with a token this machine no longer has until it was
    /// restarted. A credential is forgotten because somebody wanted it to stop
    /// working, so "until the next restart" is the wrong answer; the CLI tells
    /// the daemon over the control socket and this is what it reaches.
    ///
    /// Every host is dropped, not the one host this process would resolve: the
    /// cache holds a slot per identity per host, and the delete the CLI just
    /// performed is not scoped to one either. The prefix is exact - the key
    /// puts a unit separator after the name, and
    /// [`crystalline_remote::valid_identity_name`] keeps that byte out of
    /// names - so `alice` can never evict `alice2`.
    ///
    /// Read-only is not consulted: forgetting a cached secret is not a
    /// mutation this instance serves, and refusing it would leave the token in
    /// memory precisely where the instance can still write with it.
    ///
    /// The pending device-flow record for the same identity is dropped too,
    /// exactly as [`Engine::disconnect_github_identity`] drops it. That frees
    /// the one-flow-at-a-time slot and forgets the flow's outcome; it does not
    /// stop the spawned exchange itself, which on a later completion still
    /// saves its token and re-fills the cache - a residue both disconnect
    /// paths share (see plans/backlog.md), narrow because that flow was
    /// user-initiated moments earlier.
    pub fn forget_cached_credential(&self, account: Option<&str>) -> Result<()> {
        let identity = match account {
            None => TokenIdentity::Instance,
            Some(name) => personal_identity(name)?,
        };
        let prefix = credential_cache_key(&identity, None);
        self.github_tokens
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(&prefix));
        self.clear_pending_for(&identity);
        Ok(())
    }
}
