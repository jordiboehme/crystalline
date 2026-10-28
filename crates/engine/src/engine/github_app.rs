//! The GitHub App at the credential seam: which App this instance runs (from
//! the environment or the App store), and [`AppRouter`], the [`Provider`]
//! that sends every call for a repository on that repository's installation
//! token.
//!
//! With no App configured nothing here is reached beyond one store read, and
//! every origin operation builds its provider exactly as before.

use std::path::PathBuf;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crystalline_remote::github::{GitHubProvider, HttpClient};
use crystalline_remote::provider::{
    CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, StackInfo, TreeWrite,
};
use crystalline_remote::{
    AppCredentials, AppLookup, AppStore, CommitIdentities, CommitIdentity, GithubApp, RemoteError,
};
use serde::{Deserialize, Serialize};

use super::*;

/// A stand-in for the App store read, for tests (see
/// `Engine::with_app_store_lookup`).
#[cfg(any(test, feature = "testing"))]
pub type AppLookupFn = Arc<dyn Fn() -> StdResult<AppLookup, RemoteError> + Send + Sync>;

/// The clock the App memory reads (see `Engine::with_app_clock`): the
/// moment a failed keychain read is measured against.
pub type AppClock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// How long a failed keychain read is remembered before the keychain is
/// asked again. A keychain that hangs costs up to its read timeout per ask,
/// so asking on every origin operation would stall each one.
const UNREADABLE_FOR: Duration = Duration::from_secs(60);

/// What an origin operation is refused with while the keychain that holds
/// the App (the marker says so) cannot be read.
pub(crate) const KEYCHAIN_REFUSAL: &str =
    "the GitHub App is saved in the keychain, which could not be read; try again or unlock it";

/// What `set` and `remove` are refused with while the environment supplies
/// the App.
pub const APP_FROM_ENV: &str = "the GitHub App comes from the CRYSTALLINE_GITHUB_APP_* environment variables, which win over a saved App; unset them to set or remove a saved one";

/// A GitHub App as `crystalline github app status` and the `configure` tool
/// report it. It names the App and says which secrets are set; it never
/// holds a secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubAppStatus {
    /// Whether an App is configured at all, usable or not.
    pub configured: bool,
    /// Where it comes from: `"environment"`, `"keyring"` or `"file"`.
    pub source: Option<String>,
    /// The numeric App id.
    pub app_id: Option<u64>,
    /// The App's client id.
    pub client_id: Option<String>,
    /// The App's slug, when known.
    pub slug: Option<String>,
    /// Whether a client secret is set.
    pub client_secret_set: bool,
    /// Whether a webhook secret is set.
    pub webhook_secret_set: bool,
    /// The accounts the App is installed on, by login. `None` when GitHub
    /// was not asked (the `configure` tool never asks) or could not answer.
    pub installations: Option<Vec<String>>,
    /// Why the App cannot be used, or why GitHub could not be asked.
    pub error: Option<String>,
}

impl GithubAppStatus {
    /// The status of a configured App, from its credentials.
    fn of(credentials: &AppCredentials, source: &str) -> GithubAppStatus {
        GithubAppStatus {
            configured: true,
            source: Some(source.to_string()),
            app_id: Some(credentials.app_id),
            client_id: Some(credentials.client_id.clone()),
            slug: credentials.slug.clone(),
            client_secret_set: credentials.client_secret.is_some(),
            webhook_secret_set: credentials.webhook_secret.is_some(),
            installations: None,
            error: None,
        }
    }

    /// Asks GitHub for the slug and the installations; a failure lands in
    /// `error` and never fails the status.
    async fn ask_github(mut self, app: &GithubApp) -> GithubAppStatus {
        match app.app_info().await {
            Ok(info) => self.slug = Some(info.slug),
            Err(e) => {
                self.error = Some(e.to_string());
                return self;
            }
        }
        match app.installations().await {
            Ok(accounts) => self.installations = Some(accounts),
            Err(e) => self.error = Some(e.to_string()),
        }
        self
    }
}

/// Where one configuration finds and keeps its GitHub App: the effective
/// `github.api_url`, and the state directory the file store and the
/// keychain marker live in. The engine builds one for itself; the CLI builds
/// one to set or remove an App without opening an engine, so both go through
/// the same code.
#[derive(Clone, Debug)]
pub struct AppHome {
    /// The effective `github.api_url`.
    pub api_url: Option<String>,
    /// The state directory beside the token store.
    pub dir: PathBuf,
    /// Only the file under `dir`, never the keychain: the test seam.
    pub file_only: bool,
}

impl AppHome {
    /// The host the App is saved for: `github.com` or the Enterprise Server
    /// host of `api_url`.
    fn host(&self) -> String {
        origin::token_host(self.api_url.as_deref()).unwrap_or_else(|| "github.com".to_string())
    }

    /// One read of the App store for this home.
    pub fn lookup(&self) -> StdResult<AppLookup, RemoteError> {
        if self.file_only {
            let store = AppStore::file_fallback(&self.dir);
            let credentials = store.load()?;
            return Ok(AppLookup {
                store,
                credentials,
                keychain_failed: None,
                marker: None,
            });
        }
        AppStore::lookup(
            origin::token_host(self.api_url.as_deref()).as_deref(),
            &self.dir,
        )
    }

    /// The App as this home finds it, read afresh, with GitHub asked for
    /// its slug and installations. What the CLI reports when no daemon is
    /// running.
    pub async fn status(&self, overlay: &EnvOverlay) -> GithubAppStatus {
        let (credentials, source) = match overlay.github_app(self.api_url.as_deref()) {
            Some(Ok(credentials)) => (credentials, "environment"),
            Some(Err(detail)) => return refused_status(Some("environment"), detail),
            None => match self.lookup() {
                Ok(AppLookup {
                    credentials: Some(credentials),
                    store,
                    ..
                }) => (credentials, store.kind()),
                Ok(AppLookup {
                    keychain_failed: Some(reason),
                    marker,
                    ..
                }) => return unreadable_status(&reason, marker.is_some()),
                Ok(_) => return GithubAppStatus::default(),
                Err(e) => return refused_status(None, detail_of(e)),
            },
        };
        let status = GithubAppStatus::of(&credentials, source);
        match GithubApp::new(self.api_url.clone(), credentials) {
            Ok(app) => status.ask_github(&app).await,
            Err(e) => GithubAppStatus {
                error: Some(e.to_string()),
                ..status
            },
        }
    }

    /// Validates `credentials` against GitHub (`GET /app` with the App's
    /// signed token) and saves them: in the keychain with its marker when
    /// the keychain works, else in the file. The slug and the host are
    /// taken from GitHub and the effective `github.api_url`, whatever the
    /// caller passed. Refused while the environment supplies the App, and
    /// when GitHub knows the key as another App.
    pub async fn save(
        &self,
        overlay: &EnvOverlay,
        mut credentials: AppCredentials,
    ) -> Result<GithubAppStatus> {
        if overlay.github_app(self.api_url.as_deref()).is_some() {
            return Err(EngineError::Conflict(APP_FROM_ENV.to_string()));
        }
        credentials.host = self.host();
        credentials.slug = None;
        let app = GithubApp::new(self.api_url.clone(), credentials.clone())?;
        let info = app.app_info().await?;
        if info.id != credentials.app_id {
            return Err(EngineError::Invalid(format!(
                "GitHub knows this private key and client id as App {}, not App {}; check the App id",
                info.id, credentials.app_id
            )));
        }
        credentials.slug = Some(info.slug);
        let found = self.lookup()?;
        // The keychain holds an App it cannot hand over right now: a save to
        // the file would be shadowed by that App once the keychain answers.
        if let (Some(reason), Some(_)) = (&found.keychain_failed, &found.marker) {
            return Err(EngineError::Conflict(format!(
                "the keychain could not be read ({reason}), and a GitHub App is saved there; unlock it and run the set again"
            )));
        }
        found.store.save_marked(&credentials, &self.dir)?;
        let status = GithubAppStatus::of(&credentials, found.store.kind());
        Ok(match app.installations().await {
            Ok(accounts) => GithubAppStatus {
                installations: Some(accounts),
                ..status
            },
            Err(e) => GithubAppStatus {
                error: Some(e.to_string()),
                ..status
            },
        })
    }

    /// Deletes the saved App, its keychain marker and any copy in the file.
    /// Refused while the environment supplies the App, and while the
    /// keychain cannot be read (an App saved there would stay behind).
    pub fn remove(&self, overlay: &EnvOverlay) -> Result<()> {
        if overlay.github_app(self.api_url.as_deref()).is_some() {
            return Err(EngineError::Conflict(APP_FROM_ENV.to_string()));
        }
        let found = self.lookup()?;
        if let Some(reason) = found.keychain_failed {
            return Err(EngineError::Conflict(format!(
                "the keychain could not be read ({reason}), so a GitHub App saved there cannot be removed; unlock it and run the remove again"
            )));
        }
        found.store.delete_marked(&self.dir)?;
        if !matches!(found.store, AppStore::File { .. }) {
            AppStore::file_fallback(&self.dir).delete_marked(&self.dir)?;
        }
        Ok(())
    }
}

/// The error an origin operation is refused with under a `Refused` slot:
/// the ruling's own sentence while the keychain holding the App cannot be
/// read (a locked keychain is not a bad key), else the App credential
/// error.
pub(crate) fn app_refusal(detail: String) -> EngineError {
    if detail == KEYCHAIN_REFUSAL {
        EngineError::Conflict(detail)
    } else {
        RemoteError::AppCredential(detail).into()
    }
}

/// The status of an App that is configured but cannot be used.
fn refused_status(source: Option<&str>, detail: String) -> GithubAppStatus {
    GithubAppStatus {
        configured: true,
        source: source.map(str::to_string),
        error: Some(RemoteError::AppCredential(detail).to_string()),
        ..GithubAppStatus::default()
    }
}

/// The status while the keychain cannot be read: an App saved there (the
/// marker says so) is configured and refused; without the marker nothing
/// is known to be saved.
fn unreadable_status(reason: &str, saved: bool) -> GithubAppStatus {
    if saved {
        GithubAppStatus {
            configured: true,
            source: Some("keyring".to_string()),
            error: Some(format!("{KEYCHAIN_REFUSAL} ({reason})")),
            ..GithubAppStatus::default()
        }
    } else {
        GithubAppStatus {
            error: Some(format!(
                "the keychain could not be read ({reason}); a GitHub App saved there is not seen until it answers"
            )),
            ..GithubAppStatus::default()
        }
    }
}

/// The App slot with what it was loaded for. A slot belongs to the
/// effective `github.api_url` it was loaded under: when that changes, the
/// slot is loaded again, so an App bound to an old host is never kept.
pub(crate) struct AppState {
    slot: AppSlot,
    /// The effective `github.api_url` this state was loaded for.
    api_url: Option<String>,
    /// Where the App came from, for the status.
    source: Option<&'static str>,
    /// A failed keychain read, remembered for [`UNREADABLE_FOR`] while the
    /// slot stays `Unloaded`.
    unreadable: Option<Unreadable>,
}

/// A keychain read that failed, and whether the marker says an App is
/// saved in that keychain for this host.
struct Unreadable {
    at: Instant,
    reason: String,
    saved: bool,
}

impl Unreadable {
    /// What an origin operation gets while this is remembered: refused when
    /// an App is saved in the keychain, else the instance credential as
    /// without an App.
    fn answer(&self) -> AppSlot {
        if self.saved {
            AppSlot::Refused(KEYCHAIN_REFUSAL.to_string())
        } else {
            AppSlot::Absent
        }
    }
}

impl AppState {
    pub(crate) fn new() -> AppState {
        AppState {
            slot: AppSlot::Unloaded,
            api_url: None,
            source: None,
            unreadable: None,
        }
    }

    /// The settled answer for `api_url` at `now`, or `None` when the App
    /// has to be looked for (never looked for, looked for under another
    /// `github.api_url`, or a remembered keychain failure ran out).
    fn settled(&self, api_url: &Option<String>, now: Instant) -> Option<AppSlot> {
        if self.api_url != *api_url {
            return None;
        }
        if !matches!(self.slot, AppSlot::Unloaded) {
            return Some(self.slot.clone());
        }
        self.unreadable
            .as_ref()
            .filter(|u| now.saturating_duration_since(u.at) < UNREADABLE_FOR)
            .map(Unreadable::answer)
    }

    /// The router of a loaded, usable App, whatever `github.api_url` says
    /// now. Used only to forget the App's caches, never to send.
    pub(crate) fn ready(&self) -> Option<&Arc<AppRouter>> {
        match &self.slot {
            AppSlot::Ready(router) => Some(router),
            _ => None,
        }
    }
}

/// What this engine knows about its GitHub App. Read once, on the first
/// origin operation that needs a credential, and kept.
#[derive(Clone)]
pub(crate) enum AppSlot {
    /// Not looked for yet.
    Unloaded,
    /// No App is configured: every call goes out on the instance credential,
    /// as before the App existed.
    Absent,
    /// An App is configured and usable.
    Ready(Arc<AppRouter>),
    /// An App is configured but cannot be used (a saved App for another
    /// host, a private key that does not parse, a store that cannot be
    /// read). Every origin operation is refused with this detail rather than
    /// falling back to the instance credential, because which repositories
    /// the App covers is unknown.
    Refused(String),
}

/// A [`Provider`] that sends each call on the installation token of the
/// repository it is about.
///
/// Holds one HTTP client, cloned into every per-call [`GitHubProvider`]: a
/// client build loads the platform trust store (see the note above
/// `Engine::resolve_share_credential`), so it is built once per App, not
/// once per call. A call refused with [`RemoteError::AuthExpired`] (a 401:
/// the token was revoked or expired early) forgets that installation's token
/// and runs once more on a fresh one. A 401 means GitHub did not act on the
/// request, so the retry never writes anything twice.
pub(crate) struct AppRouter {
    app: Arc<GithubApp>,
    api_url: Option<String>,
    /// Who the commits made through this router name as author and
    /// committer. `None` on the router the slot keeps, which every pull, the
    /// Reflection backup and an owner's share use, so the bot authors those;
    /// set only on the per-share copy [`AppRouter::with_commit`] makes.
    commit: Option<CommitIdentities>,
    client: HttpClient,
}

impl AppRouter {
    pub(crate) fn new(app: Arc<GithubApp>, api_url: Option<String>) -> AppRouter {
        AppRouter {
            app,
            api_url,
            commit: None,
            client: GitHubProvider::http_client(),
        }
    }

    /// A copy of this router whose commits name `identities` as author and
    /// committer. A copy, never a change to the slot's router: that one is
    /// shared by every operation, and a person's name must not leak into
    /// another share or the backup. The copy shares the App's caches and
    /// the HTTP client.
    pub(crate) fn with_commit(&self, identities: CommitIdentities) -> AppRouter {
        AppRouter {
            app: self.app.clone(),
            api_url: self.api_url.clone(),
            commit: Some(identities),
            client: self.client.clone(),
        }
    }

    /// The App this router sends through.
    pub(crate) fn app(&self) -> &Arc<GithubApp> {
        &self.app
    }

    /// The login the App's proposals are opened as, when its slug is known
    /// without a request.
    pub(crate) fn bot_login(&self) -> Option<String> {
        self.app.known_slug().map(|slug| format!("{slug}[bot]"))
    }

    fn provider(&self, token: Option<String>) -> GitHubProvider {
        let provider =
            GitHubProvider::with_client(self.client.clone(), self.api_url.clone(), token);
        match &self.commit {
            Some(identities) => provider.with_commit_identities(identities.clone()),
            None => provider,
        }
    }

    /// Forgets the token `repo` was just refused with, so the next
    /// [`GithubApp::token_for_repo`] mints a new one. The installation id is
    /// served from the cache the refused call filled, so this sends nothing.
    async fn forget_token(&self, repo: &str) {
        match self.app.installation_for(repo).await {
            Ok(id) => self.app.evict_installation(id),
            Err(_) => self.app.evict_repo(repo),
        }
    }
}

/// Runs `$call` (an expression over `$provider`) on `$repo`'s installation
/// token, once more on a renewed token after an `AuthExpired`.
macro_rules! on_installation_token {
    ($router:expr, $repo:expr, |$provider:ident| $call:expr) => {{
        let router: &AppRouter = $router;
        let repo: &str = $repo;
        let $provider = router.provider(Some(router.app.token_for_repo(repo).await?));
        match $call.await {
            Err(RemoteError::AuthExpired) => {
                router.forget_token(repo).await;
                let $provider = router.provider(Some(router.app.token_for_repo(repo).await?));
                $call.await
            }
            other => other,
        }
    }};
}

#[async_trait]
impl Provider for AppRouter {
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        etag: Option<&str>,
    ) -> StdResult<HeadProbe, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.branch_head(origin, etag))
    }

    async fn compare(
        &self,
        origin: &OriginSpec,
        base: &str,
        head: &str,
    ) -> StdResult<CompareResult, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.compare(origin, base, head))
    }

    async fn blob(&self, origin: &OriginSpec, sha: &str) -> StdResult<Vec<u8>, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.blob(origin, sha))
    }

    async fn read_file(
        &self,
        origin: &OriginSpec,
        reference: &str,
        path: &str,
    ) -> StdResult<Option<Vec<u8>>, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.read_file(origin, reference, path))
    }

    async fn tarball(&self, origin: &OriginSpec, commit: &str) -> StdResult<Vec<u8>, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.tarball(origin, commit))
    }

    async fn create_blob(
        &self,
        origin: &OriginSpec,
        content: &[u8],
    ) -> StdResult<String, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.create_blob(origin, content))
    }

    async fn create_tree(
        &self,
        origin: &OriginSpec,
        parent_commit: &str,
        writes: &[TreeWrite],
    ) -> StdResult<String, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.create_tree(
            origin,
            parent_commit,
            writes
        ))
    }

    async fn create_root_tree(
        &self,
        origin: &OriginSpec,
        writes: &[TreeWrite],
    ) -> StdResult<String, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.create_root_tree(origin, writes))
    }

    async fn create_commit(
        &self,
        origin: &OriginSpec,
        message: &str,
        tree: &str,
        parents: &[String],
    ) -> StdResult<String, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p
            .create_commit(origin, message, tree, parents))
    }

    async fn create_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
    ) -> StdResult<(), RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p
            .create_branch(origin, name, commit))
    }

    async fn delete_branch(&self, origin: &OriginSpec, name: &str) -> StdResult<(), RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.delete_branch(origin, name))
    }

    async fn branch_ref(
        &self,
        origin: &OriginSpec,
        name: &str,
    ) -> StdResult<Option<String>, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.branch_ref(origin, name))
    }

    async fn update_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
        force: bool,
    ) -> StdResult<(), RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p
            .update_branch(origin, name, commit, force))
    }

    fn commit_url(&self, origin: &OriginSpec, sha: &str) -> Option<String> {
        // Pure: derived from the API base, so no token is needed.
        self.provider(None).commit_url(origin, sha)
    }

    async fn update_proposal(
        &self,
        origin: &OriginSpec,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        base: Option<&str>,
    ) -> StdResult<(), RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p
            .update_proposal(origin, number, title, body, base))
    }

    async fn close_proposal(&self, origin: &OriginSpec, number: u64) -> StdResult<(), RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.close_proposal(origin, number))
    }

    async fn proposal_feedback(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> StdResult<Feedback, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.proposal_feedback(origin, number))
    }

    async fn list_open_proposals(
        &self,
        origin: &OriginSpec,
    ) -> StdResult<Vec<OpenProposalRef>, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.list_open_proposals(origin))
    }

    async fn create_proposal(
        &self,
        origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> StdResult<ProposalHandle, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.create_proposal(origin, req))
    }

    async fn proposal_state(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> StdResult<ProposalState, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.proposal_state(origin, number))
    }

    /// The App's bot login, `<slug>[bot]`: an installation token belongs to
    /// no user, so `GET /user` has nobody to answer with.
    async fn current_user(&self) -> StdResult<String, RemoteError> {
        Ok(format!("{}[bot]", self.app.slug().await?))
    }

    async fn default_branch(&self, repo: &str) -> StdResult<String, RemoteError> {
        on_installation_token!(self, repo, |p| p.default_branch(repo))
    }

    async fn list_stacks(
        &self,
        origin: &OriginSpec,
        pull_request: Option<u64>,
    ) -> StdResult<Vec<StackInfo>, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.list_stacks(origin, pull_request))
    }

    async fn create_stack(
        &self,
        origin: &OriginSpec,
        pull_requests: &[u64],
    ) -> StdResult<StackInfo, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p
            .create_stack(origin, pull_requests))
    }

    async fn extend_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
        pull_requests: &[u64],
    ) -> StdResult<StackInfo, RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p.extend_stack(
            origin,
            stack_number,
            pull_requests
        ))
    }

    async fn dissolve_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
    ) -> StdResult<(), RemoteError> {
        on_installation_token!(self, &origin.repo, |p| p
            .dissolve_stack(origin, stack_number))
    }
}

impl Engine {
    /// The GitHub App this instance runs, when one is configured and usable.
    pub fn github_app(&self) -> Option<Arc<GithubApp>> {
        match self.github_app_slot() {
            AppSlot::Ready(router) => Some(router.app().clone()),
            _ => None,
        }
    }

    /// The router an `instance` share by `actor` on `repo` goes out through.
    ///
    /// Reads the App's slug first when the credentials carried none (an App
    /// from the environment), so the share records the bot's login from the
    /// very first one. An account that has a contact address is named as the
    /// commits' author, with the App's bot as committer; everyone else (the
    /// machine owner, an unauthenticated agent, an account without an
    /// address) gets the slot's own router, whose commits the bot authors.
    /// Attribution never fails a share: when the person or the bot cannot be
    /// read, a warning is logged and the bot authors the commit.
    ///
    /// The author address is the account's email as the admin, the identity
    /// provider or the forward-auth proxy supplied it. Crystalline does not
    /// verify it, and GitHub credits the commit to whichever GitHub account
    /// holds that address; an identity provider address marked
    /// `email_verified: false` is never stored, so it never gets here.
    pub(super) async fn app_share_router(
        &self,
        router: Arc<AppRouter>,
        actor: &ShareActor,
        repo: &str,
    ) -> Arc<AppRouter> {
        if let Err(e) = router.app().slug().await {
            tracing::debug!("the GitHub App's slug could not be read before a share: {e}");
        }
        let ShareActor::Account(account) = actor else {
            return router;
        };
        let Some(access) = self.domain_access.get() else {
            return router;
        };
        let contact = match access.account_contact(account).await {
            Ok(Some(contact)) => contact,
            Ok(None) => {
                tracing::debug!("no account named {account}, so the GitHub App authors its share");
                return router;
            }
            Err(e) => {
                tracing::warn!(
                    "the account {account} could not be read, so the GitHub App authors its share: {e}"
                );
                return router;
            }
        };
        // An account without an address of its own is not named; the bot
        // authors it.
        let Some(email) = contact.email else {
            tracing::debug!(
                "the account {account} has no email address, so the GitHub App authors its share"
            );
            return router;
        };
        let committer = match router.app().bot_identity(repo).await {
            Ok(bot) => bot,
            Err(e) => {
                tracing::warn!(
                    "the GitHub App's bot identity could not be read, so the App authors the share of {account}: {e}"
                );
                return router;
            }
        };
        let author = CommitIdentity {
            name: contact.display,
            email,
        };
        Arc::new(router.with_commit(CommitIdentities { author, committer }))
    }

    /// The App slot, loading it on first use and again whenever the
    /// effective `github.api_url` changed since. An injected test provider
    /// short-circuits every credential decision, so under one there is
    /// never an App and nothing is read.
    ///
    /// A keychain read that failed is remembered for a minute
    /// ([`UNREADABLE_FOR`]): in that minute the keychain is not asked again,
    /// and each call is refused when the marker says the App is saved in
    /// that keychain, or runs as without an App when it does not.
    pub(super) fn github_app_slot(&self) -> AppSlot {
        if self.origin_provider_override.is_some() {
            return AppSlot::Absent;
        }
        let api_url = self.effective_api_url();
        let now = (self.app_clock)();
        if let Some(slot) = self.github_app.read().unwrap().settled(&api_url, now) {
            return slot;
        }
        let mut state = self.github_app.write().unwrap();
        // Another caller may have loaded it while this one waited for the
        // write lock.
        if let Some(slot) = state.settled(&api_url, now) {
            return slot;
        }
        let loaded = self.load_github_app(api_url.as_deref());
        let answer = match &loaded.unreadable {
            Some((_, true)) => AppSlot::Refused(KEYCHAIN_REFUSAL.to_string()),
            Some((_, false)) => AppSlot::Absent,
            None => loaded.slot.clone(),
        };
        *state = AppState {
            slot: loaded.slot,
            api_url,
            source: loaded.source,
            unreadable: loaded.unreadable.map(|(reason, saved)| Unreadable {
                at: now,
                reason,
                saved,
            }),
        };
        answer
    }

    /// Forgets the App and any remembered keychain failure, so the next
    /// origin operation looks for it again. Called after `set` and `remove`
    /// change the saved App, here or (through the control socket) in the CLI
    /// process.
    pub fn reset_github_app(&self) {
        *self.github_app.write().unwrap() = AppState::new();
    }

    /// Why the keychain could not be read, while a failed read is
    /// remembered.
    pub(super) fn github_app_keychain_unreadable(&self) -> Option<String> {
        let now = (self.app_clock)();
        let state = self.github_app.read().unwrap();
        state
            .unreadable
            .as_ref()
            .filter(|u| now.saturating_duration_since(u.at) < UNREADABLE_FOR)
            .map(|u| u.reason.clone())
    }

    /// The effective `github.api_url`.
    fn effective_api_url(&self) -> Option<String> {
        self.config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone())
    }

    /// Where this engine finds and keeps its App: the file under the test
    /// token directory when one is set (never the keychain), else the real
    /// store, the way [`Engine::github_credential_for`] reads tokens.
    pub(super) fn app_home(&self) -> Result<AppHome> {
        Ok(match &self.token_store_dir_override {
            Some(dir) => AppHome {
                api_url: self.effective_api_url(),
                dir: dir.clone(),
                file_only: true,
            },
            None => AppHome {
                api_url: self.effective_api_url(),
                dir: self.origins_base_dir()?,
                file_only: false,
            },
        })
    }

    /// The App as this engine runs it, without asking GitHub: for the
    /// `configure` tool, which agents call often.
    pub fn github_app_local_status(&self) -> GithubAppStatus {
        if self.origin_provider_override.is_some() {
            return GithubAppStatus::default();
        }
        let slot = self.github_app_slot();
        let source = self.github_app.read().unwrap().source;
        match slot {
            AppSlot::Ready(router) => {
                let mut status =
                    GithubAppStatus::of(router.app().credentials(), source.unwrap_or("keyring"));
                status.slug = router.app().known_slug();
                status
            }
            AppSlot::Refused(detail) if detail == KEYCHAIN_REFUSAL => unreadable_status(
                &self.github_app_keychain_unreadable().unwrap_or_default(),
                true,
            ),
            AppSlot::Refused(detail) => refused_status(source, detail),
            AppSlot::Absent | AppSlot::Unloaded => match self.github_app_keychain_unreadable() {
                Some(reason) => unreadable_status(&reason, false),
                None => GithubAppStatus::default(),
            },
        }
    }

    /// The App as this engine runs it, with GitHub asked for its slug and
    /// the accounts it is installed on.
    pub async fn github_app_status(&self) -> GithubAppStatus {
        let status = self.github_app_local_status();
        match self.github_app_slot() {
            AppSlot::Ready(router) => status.ask_github(router.app()).await,
            _ => status,
        }
    }

    /// Validates and saves `credentials` as this instance's App (see
    /// [`AppHome::save`]) and drops the App the engine held, so the next
    /// origin operation runs on the new one.
    pub async fn set_github_app(&self, credentials: AppCredentials) -> Result<GithubAppStatus> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let saved = self.app_home()?.save(&self.overlay, credentials).await;
        self.reset_github_app();
        saved
    }

    /// Removes the saved App (see [`AppHome::remove`]) and drops the App the
    /// engine held: origin operations run on the instance credential again.
    pub async fn remove_github_app(&self) -> Result<()> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let removed = self.app_home()?.remove(&self.overlay);
        self.reset_github_app();
        removed
    }

    /// Finds the App: the `CRYSTALLINE_GITHUB_APP_*` variables win over a
    /// saved App. Either goes through [`GithubApp::new`] against the
    /// effective `github.api_url`, so a saved App for another host is refused
    /// (and said so in the log and in `origin status`) rather than used.
    fn load_github_app(&self, api_url: Option<&str>) -> Loaded {
        let (source, credentials) = match self.overlay.github_app(api_url) {
            Some(Ok(credentials)) => ("environment", credentials),
            Some(Err(detail)) => {
                return Loaded::slot(refused("the environment", detail), Some("environment"));
            }
            None => match self.saved_github_app() {
                Ok(AppLookup {
                    credentials: Some(credentials),
                    store,
                    ..
                }) => (store.kind(), credentials),
                Ok(AppLookup {
                    credentials: None,
                    keychain_failed: None,
                    ..
                }) => return Loaded::slot(AppSlot::Absent, None),
                // The keychain did not answer (locked, slow, or no keychain
                // on this machine at all) and the file holds no App. The
                // marker says whether an App was saved in that keychain for
                // this host: then every operation is refused until it
                // answers; without it the call runs as without an App. Either
                // way the failure is remembered for a minute, not for good.
                Ok(AppLookup {
                    credentials: None,
                    keychain_failed: Some(reason),
                    marker,
                    ..
                }) => {
                    let host =
                        origin::token_host(api_url).unwrap_or_else(|| "github.com".to_string());
                    // An unreadable marker names no host and still counts.
                    let saved = marker.is_some_and(|marked| {
                        marked.is_empty() || marked.eq_ignore_ascii_case(&host)
                    });
                    if saved {
                        tracing::warn!(
                            "the keychain could not be read ({reason}); the GitHub App saved there is not used, and origin operations are refused until it answers"
                        );
                    } else {
                        tracing::debug!(
                            "no GitHub App in the file store and the keychain could not be read ({reason}); asking again in a minute"
                        );
                    }
                    return Loaded {
                        slot: AppSlot::Unloaded,
                        source: Some("keyring"),
                        unreadable: Some((reason, saved)),
                    };
                }
                Err(e) => return Loaded::slot(refused("the App store", detail_of(e)), None),
            },
        };
        let api_url = api_url.map(str::to_string);
        let slot = match GithubApp::new(api_url.clone(), credentials) {
            Ok(app) => {
                tracing::info!("GitHub App loaded from the {source}");
                AppSlot::Ready(Arc::new(AppRouter::new(Arc::new(app), api_url)))
            }
            Err(e) => refused(source, detail_of(e)),
        };
        Loaded::slot(slot, Some(source))
    }

    /// The saved App for the effective host, from the test lookup when one
    /// is injected, else through [`AppHome::lookup`].
    fn saved_github_app(&self) -> StdResult<AppLookup, RemoteError> {
        #[cfg(any(test, feature = "testing"))]
        if let Some(lookup) = &self.app_store_lookup {
            return lookup();
        }
        self.app_home()
            .map_err(|e| RemoteError::AppCredential(e.to_string()))?
            .lookup()
    }
}

/// What one App load found.
struct Loaded {
    /// The slot to keep; `Unloaded` after a failed keychain read.
    slot: AppSlot,
    /// Where the App came from.
    source: Option<&'static str>,
    /// A failed keychain read: why, and whether an App is saved there.
    unreadable: Option<(String, bool)>,
}

impl Loaded {
    fn slot(slot: AppSlot, source: Option<&'static str>) -> Loaded {
        Loaded {
            slot,
            source,
            unreadable: None,
        }
    }
}

/// The detail an App refusal carries: the [`RemoteError::AppCredential`]
/// text itself, so it is not wrapped twice when it is raised again.
fn detail_of(e: RemoteError) -> String {
    match e {
        RemoteError::AppCredential(detail) => detail,
        other => other.to_string(),
    }
}

fn refused(source: &str, detail: String) -> AppSlot {
    tracing::warn!(
        "the GitHub App from {source} is not used, and origin operations are refused until it is fixed: {detail}"
    );
    AppSlot::Refused(detail)
}
