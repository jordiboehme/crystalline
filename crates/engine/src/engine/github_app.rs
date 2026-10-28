//! The GitHub App at the credential seam: which App this instance runs (from
//! the environment or the App store), and [`AppRouter`], the [`Provider`]
//! that sends every call for a repository on that repository's installation
//! token.
//!
//! With no App configured nothing here is reached beyond one store read, and
//! every origin operation builds its provider exactly as before.

use std::result::Result as StdResult;
use std::sync::Arc;

use async_trait::async_trait;
use crystalline_remote::github::{GitHubProvider, HttpClient};
use crystalline_remote::provider::{
    CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, StackInfo, TreeWrite,
};
use crystalline_remote::{
    AppLookup, AppStore, CommitIdentities, CommitIdentity, GithubApp, RemoteError,
};

use super::*;

/// A stand-in for the App store read, for tests (see
/// `Engine::with_app_store_lookup`).
#[cfg(any(test, feature = "testing"))]
pub type AppLookupFn = Arc<dyn Fn() -> StdResult<AppLookup, RemoteError> + Send + Sync>;

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

    /// The App slot, loading it on first use. An injected test provider
    /// short-circuits every credential decision, so under one there is
    /// never an App and nothing is read.
    pub(super) fn github_app_slot(&self) -> AppSlot {
        if self.origin_provider_override.is_some() {
            return AppSlot::Absent;
        }
        {
            let slot = self.github_app.read().unwrap();
            if !matches!(*slot, AppSlot::Unloaded) {
                return slot.clone();
            }
        }
        let mut slot = self.github_app.write().unwrap();
        // Another caller may have loaded it while this one waited for the
        // write lock.
        if !matches!(*slot, AppSlot::Unloaded) {
            return slot.clone();
        }
        match self.load_github_app() {
            Loaded::Keep(loaded) => {
                *slot = loaded.clone();
                loaded
            }
            // Left `Unloaded`, so the next call asks the keychain again.
            Loaded::ThisCallOnly(loaded) => loaded,
        }
    }

    /// Finds the App: the `CRYSTALLINE_GITHUB_APP_*` variables win over a
    /// saved App. Either goes through [`GithubApp::new`] against the
    /// effective `github.api_url`, so a saved App for another host is refused
    /// (and said so in the log and in `origin status`) rather than used.
    fn load_github_app(&self) -> Loaded {
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let (source, credentials) = match self.overlay.github_app(api_url.as_deref()) {
            Some(Ok(credentials)) => ("the environment", credentials),
            Some(Err(detail)) => return Loaded::Keep(refused("the environment", detail)),
            None => match self.saved_github_app(api_url.as_deref()) {
                Ok(AppLookup {
                    credentials: Some(credentials),
                    ..
                }) => ("the App store", credentials),
                Ok(AppLookup {
                    credentials: None,
                    keychain_failed: None,
                    ..
                }) => return Loaded::Keep(AppSlot::Absent),
                // The keychain did not answer (locked, slow, or no keychain
                // on this machine at all) and the file holds no App. An App
                // saved in that keychain would read exactly like this, so the
                // absence is not kept: the next call asks again, the way the
                // token cache never keeps a missing token. The cost is one
                // bounded keychain read per origin operation while it fails.
                Ok(AppLookup {
                    credentials: None,
                    keychain_failed: Some(reason),
                    ..
                }) => {
                    tracing::debug!(
                        "no GitHub App in the file store and the keychain could not be read \
                         ({reason}); asking again on the next origin operation"
                    );
                    return Loaded::ThisCallOnly(AppSlot::Absent);
                }
                Err(e) => return Loaded::Keep(refused("the App store", detail_of(e))),
            },
        };
        Loaded::Keep(match GithubApp::new(api_url.clone(), credentials) {
            Ok(app) => {
                tracing::info!("GitHub App loaded from {source}");
                AppSlot::Ready(Arc::new(AppRouter::new(Arc::new(app), api_url)))
            }
            Err(e) => refused(source, detail_of(e)),
        })
    }

    /// The saved App for the effective host: from the file under the test
    /// token directory when one is set (never the keychain), else the real
    /// store, the way [`Engine::github_credential_for`] reads tokens.
    fn saved_github_app(&self, api_url: Option<&str>) -> StdResult<AppLookup, RemoteError> {
        #[cfg(any(test, feature = "testing"))]
        if let Some(lookup) = &self.app_store_lookup {
            return lookup();
        }
        match &self.token_store_dir_override {
            Some(dir) => {
                let store = AppStore::file_fallback(dir);
                let credentials = store.load()?;
                Ok(AppLookup {
                    store,
                    credentials,
                    keychain_failed: None,
                })
            }
            None => {
                let base = self
                    .origins_base_dir()
                    .map_err(|e| RemoteError::AppCredential(e.to_string()))?;
                let host = origin::token_host(api_url);
                AppStore::lookup(host.as_deref(), &base)
            }
        }
    }
}

/// What one App load found, and whether it may be kept for the process.
enum Loaded {
    /// A settled answer: kept in the slot.
    Keep(AppSlot),
    /// An answer for this call only; the slot stays `Unloaded`.
    ThisCallOnly(AppSlot),
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
