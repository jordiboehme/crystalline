//! Who may read and write a domain connected to GitHub through the GitHub
//! App: [`GithubGovernor`], the [`KnowledgeGovernor`] the engine installs
//! into every [`DomainAccess`] it serves through.
//!
//! **What is governed.** A domain is governed when an App is configured
//! (usable or not) and the domain has an `origin`, decided from the
//! configuration alone, so a domain is governed from the first request on.
//! Only a known answer that the App is not installed on the repository
//! takes a domain out; a repository whose installation was not looked up
//! yet reads as governed, with no access for anybody, until the lookup
//! lands. An App that is configured but cannot be used governs every origin
//! domain and grants nothing on any of them.
//!
//! **One listing per repository.** Everything the governor answers about a
//! repository comes from one snapshot: one paged listing of its
//! collaborators (`affiliation=all`) and whether it is public. The listing
//! is joined to the accounts by numeric GitHub id, never by login, so a
//! renamed GitHub account keeps its access, and a login the listing shows
//! for a linked id is stored as that link's login. [`GithubGovernor::knowledge`]
//! and [`GithubGovernor::readers`] both read the snapshot through
//! [`Snapshot::level_of`], so the two can never disagree. On a public
//! repository every linked account reads (GitHub answers `read` for a
//! non-collaborator there), so it is at least [`MemberLevel::Viewer`].
//!
//! **Five minutes fresh, one hour stale.** A snapshot is used for
//! [`FRESH_FOR`] (a "no access" answer too: it is part of the listing).
//! After that the next question asks GitHub again. When GitHub cannot
//! answer, the last snapshot keeps serving until [`STALE_FOR`] past its
//! expiry; after that every answer on the repository is no access. A
//! question with no snapshot at all is no access as well: the governor
//! fails closed. From the first failed lookup to the next good one the
//! governor reports itself degraded ([`GithubGovernor::degraded_since`]).
//!
//! What a failed lookup does depends on what GitHub said, see
//! [`Outcome::of`]: the App not being installed is an answer, a repository
//! GitHub no longer shows is an answer of no access, and everything else is
//! a failure to learn, which serves the stale snapshot inside the window.
//!
//! Account names and links come from the accounts store on every question,
//! never from the cache, so unlinking or disabling an account takes effect
//! on the next request.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crystalline_remote::{Collaborator, GithubApp, RemoteError, RepoRole};
use tokio::time::Instant;

use crate::auth_store::{GithubLink, MemberLevel};
use crate::scope::{DomainAccess, DomainVisibility, GithubKnowledge, KnowledgeGovernor, Scope};

/// How long one snapshot of a repository is used before GitHub is asked
/// again.
pub const FRESH_FOR: Duration = Duration::from_secs(5 * 60);

/// How long past [`FRESH_FOR`] the last snapshot keeps serving while GitHub
/// cannot be asked. After that every answer on the repository is no access.
pub const STALE_FOR: Duration = Duration::from_secs(60 * 60);

/// How long after a failed lookup the governor waits before it asks GitHub
/// about that repository again, so an outage or a rate limit is not met with
/// one request per question. In that pause the answer is what the failed
/// lookup gave: the stale snapshot inside the window, else no access.
pub const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);

/// The level a GitHub repository role gives on a governed domain: admin and
/// maintain manage, write edits, triage and read view, no access is none.
/// The one home of this table.
pub fn level_for(role: RepoRole) -> MemberLevel {
    match role {
        RepoRole::Admin | RepoRole::Maintain => MemberLevel::Manager,
        RepoRole::Write => MemberLevel::Editor,
        RepoRole::Triage | RepoRole::Read => MemberLevel::Viewer,
        RepoRole::None => MemberLevel::None,
    }
}

/// Who reads a repository on GitHub, for the Reflection source check
/// (Task 3.7): whether it is public, and the GitHub ids with read or more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoReaders {
    /// Whether the repository is public, so everybody reads it.
    pub public: bool,
    /// The GitHub ids of every collaborator with read or more.
    pub github_ids: HashSet<u64>,
}

/// What a webhook (or a change here) says is no longer true, so the
/// governor forgets it. The stale fallback is forgotten with it: after an
/// invalidation nothing old is served, even while GitHub is down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalidate {
    /// One repository (`owner/name`): its snapshot and whether the App is
    /// installed on it.
    Repo(String),
    /// One GitHub user. Which repositories they reach is not known here, so
    /// every snapshot is forgotten.
    User(u64),
    /// One installation of the App. Which repositories it covers is not
    /// known here, so every snapshot and every "not installed" answer is
    /// forgotten.
    Installation(u64),
    /// Everything.
    All,
}

/// The few GitHub App calls the governor makes, so a test can answer them
/// without a network (a paused test clock and a real HTTP client do not mix:
/// the clock jumps while a request waits and its timeout fires).
#[async_trait]
pub(crate) trait GithubSource: Send + Sync {
    /// Everyone with access to `repo`, see [`GithubApp::collaborators`].
    async fn collaborators(&self, repo: &str) -> Result<Vec<Collaborator>, RemoteError>;
    /// Whether `repo` is public, see [`GithubApp::repository_public`].
    async fn repository_public(&self, repo: &str) -> Result<bool, RemoteError>;
    /// The installation that covers `repo`, see [`GithubApp::installation_for`].
    async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError>;
}

#[async_trait]
impl GithubSource for GithubApp {
    async fn collaborators(&self, repo: &str) -> Result<Vec<Collaborator>, RemoteError> {
        GithubApp::collaborators(self, repo).await
    }

    async fn repository_public(&self, repo: &str) -> Result<bool, RemoteError> {
        GithubApp::repository_public(self, repo).await
    }

    async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError> {
        GithubApp::installation_for(self, repo).await
    }
}

/// The App as the governor sees it.
#[derive(Clone)]
pub(crate) enum GovernorApp {
    /// No App is configured: nothing is governed.
    Absent,
    /// An App is configured but cannot be used (a key that does not parse,
    /// a keychain that cannot be read). Every origin domain is governed and
    /// nobody gets anything on it.
    Unusable,
    /// A usable App.
    Ready {
        /// Where the governor asks.
        source: Arc<dyn GithubSource>,
        /// The GitHub host the App runs on, spelled as a link stores it; a
        /// link made on another host is not this host's id.
        host: String,
    },
}

/// What the governor reads from the engine. Behind a trait so the cache and
/// its clock are tested without an engine.
pub(crate) trait GovernorEnv: Send + Sync {
    /// The App, as the engine holds it now.
    fn app(&self) -> GovernorApp;
    /// Every registered domain with an origin: its name and repository.
    fn origins(&self) -> Vec<(String, String)>;
    /// The accounts store, when one is installed. Without it nobody is
    /// linked and no reader is known.
    fn access(&self) -> Option<Arc<DomainAccess>>;
}

/// One listing of a repository: whether it is public and, by GitHub id,
/// each collaborator's login and role.
#[derive(Debug)]
struct Snapshot {
    public: bool,
    people: HashMap<u64, (String, RepoRole)>,
}

impl Snapshot {
    /// The level GitHub id `id` holds on the repository: its listed role,
    /// and at least [`MemberLevel::Viewer`] on a public repository. The one
    /// function both `knowledge` and `readers` answer through.
    fn level_of(&self, id: u64) -> MemberLevel {
        let listed = self
            .people
            .get(&id)
            .map_or(MemberLevel::None, |(_, role)| level_for(*role));
        match listed {
            MemberLevel::None if self.public => MemberLevel::Viewer,
            level => level,
        }
    }
}

/// A snapshot and when it was taken.
struct Entry {
    snapshot: Arc<Snapshot>,
    fetched_at: Instant,
}

/// What the governor remembers. Behind one lock, held only between awaits.
#[derive(Default)]
struct State {
    /// By lowercased `owner/name`.
    entries: HashMap<String, Entry>,
    /// Repositories GitHub said the App is not installed on, and when it
    /// said so. Asked again in the background after [`FRESH_FOR`].
    not_installed: HashMap<String, Instant>,
    /// Repositories whose "not installed" answer is being asked again.
    rechecking: HashSet<String>,
    /// Repositories whose last lookup failed: when the failures began, and
    /// when the last one was.
    failing: HashMap<String, Failing>,
}

/// An ongoing failure to learn about one repository.
#[derive(Clone, Copy)]
struct Failing {
    /// The first failure since the last good lookup, as wall time.
    since: DateTime<Utc>,
    /// The last failed lookup, for [`RETRY_AFTER_FAILURE`].
    last: Instant,
}

/// What one failed lookup means, decided by [`Outcome::of`].
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The App is not installed on the repository: the domain is not
    /// governed. An answer, not a failure.
    NotInstalled,
    /// GitHub says the repository is not there (deleted, or not visible to
    /// the App): an answer of no access for everybody, served at once.
    Gone,
    /// GitHub could not tell: the stale snapshot serves inside the window.
    Failed,
}

impl Outcome {
    /// What `error` from a lookup means. Every variant is named, so a new
    /// one has to be decided here.
    ///
    /// * The App not installed is [`Outcome::NotInstalled`].
    /// * A repository GitHub does not show (`RepoNotFound`, a bare 404) is
    ///   [`Outcome::Gone`].
    /// * Everything else is [`Outcome::Failed`], answers GitHub could not
    ///   give: offline, a rate limit, a server fault, a 403, a redirect chain
    ///   (a transferred repository moved twice), the App's installation
    ///   token refused twice (`AuthExpired`: never the person's sign-in), a
    ///   broken App key or keychain, a listing that stopped short. They are
    ///   failures to learn, not answers, so the stale window covers them.
    ///   `UserNotFound` cannot come from the calls made here (they never
    ///   look up a login); it is a failure too, and never reaches a caller.
    fn of(error: &RemoteError) -> Outcome {
        match error {
            RemoteError::AppNotInstalled { .. } => Outcome::NotInstalled,
            RemoteError::RepoNotFound { .. } | RemoteError::Api { status: 404, .. } => {
                Outcome::Gone
            }
            RemoteError::Offline
            | RemoteError::RateLimited { .. }
            | RemoteError::Api { .. }
            | RemoteError::AuthExpired
            | RemoteError::NotConnected
            | RemoteError::AuthPending
            | RemoteError::NotEnabled
            | RemoteError::SsoAuthorizationRequired { .. }
            | RemoteError::OauthAppRestricted { .. }
            | RemoteError::UserNotFound { .. }
            | RemoteError::NotADomain { .. }
            | RemoteError::ConflictsPending { .. }
            | RemoteError::ProposalNotFound { .. }
            | RemoteError::NoWithdrawTarget { .. }
            | RemoteError::ConflictNotFound { .. }
            | RemoteError::BaseUnavailable
            | RemoteError::StacksUnsupported
            | RemoteError::Refused(_)
            | RemoteError::NotFastForward { .. }
            | RemoteError::BranchProtected { .. }
            | RemoteError::Io(_)
            | RemoteError::WorkingTree { .. }
            | RemoteError::State(_)
            | RemoteError::Credential { .. }
            | RemoteError::ServerCredential { .. }
            | RemoteError::AppCredential(_)
            | RemoteError::EndpointMissing { .. } => Outcome::Failed,
        }
    }
}

/// Locks the state, reading through a poisoned lock: every write replaces
/// whole values, so a panic elsewhere cannot leave one half written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The cache key of a repository: GitHub names are case insensitive.
fn repo_key(repo: &str) -> String {
    repo.trim().to_ascii_lowercase()
}

/// Whether `link` was made on `host`.
fn on_host(link: &GithubLink, host: &str) -> bool {
    link.host.eq_ignore_ascii_case(host)
}

/// Decides engram access on the domains connected to GitHub through the
/// GitHub App, from a five minute cache of each repository's collaborators
/// (see the module documentation).
pub struct GithubGovernor {
    env: Arc<dyn GovernorEnv>,
    /// This governor, for the background re-check of a "not installed"
    /// answer.
    me: Weak<GithubGovernor>,
    state: Mutex<State>,
    /// One lookup per repository at a time: a caller that finds the lock
    /// held waits and then reads the snapshot the holder fetched.
    fetching: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl GithubGovernor {
    /// A governor reading the App, the origins and the accounts from `env`.
    pub(crate) fn new(env: Arc<dyn GovernorEnv>) -> Arc<GithubGovernor> {
        Arc::new_cyclic(|me| GithubGovernor {
            env,
            me: me.clone(),
            state: Mutex::new(State::default()),
            fetching: Mutex::new(HashMap::new()),
        })
    }

    /// Since when lookups have been failing, or `None` while every
    /// repository's last lookup worked. Only repositories still connected to
    /// a domain count.
    pub fn degraded_since(&self) -> Option<DateTime<Utc>> {
        let connected: HashSet<String> = self
            .env
            .origins()
            .into_iter()
            .map(|(_, repo)| repo_key(&repo))
            .collect();
        lock(&self.state)
            .failing
            .iter()
            .filter(|(repo, _)| connected.contains(*repo))
            .map(|(_, failing)| failing.since)
            .min()
    }

    /// Forget what `what` names, the stale fallback included.
    pub fn invalidate(&self, what: Invalidate) {
        let mut state = lock(&self.state);
        match what {
            Invalidate::Repo(repo) => {
                let key = repo_key(&repo);
                state.entries.remove(&key);
                state.not_installed.remove(&key);
            }
            Invalidate::User(_) => state.entries.clear(),
            Invalidate::Installation(_) | Invalidate::All => {
                state.entries.clear();
                state.not_installed.clear();
            }
        }
    }

    /// Who reads `domain`'s repository on GitHub: whether it is public and
    /// the GitHub ids with read or more, from the same snapshot as every
    /// other answer. `None` when that is not known right now (no usable
    /// App, GitHub down past the window, the App not installed, a domain
    /// without an origin); every caller then fails closed.
    pub async fn repository_readers(&self, domain: &str) -> Option<RepoReaders> {
        let repo = self.repo_of(domain)?;
        let GovernorApp::Ready { source, .. } = self.env.app() else {
            return None;
        };
        let snapshot = self.snapshot(source.as_ref(), &repo).await?;
        Some(RepoReaders {
            public: snapshot.public,
            github_ids: snapshot
                .people
                .iter()
                .filter(|(_, (_, role))| level_for(*role) != MemberLevel::None)
                .map(|(id, _)| *id)
                .collect(),
        })
    }

    /// The repository of the registered domain `domain`, when it has an
    /// origin.
    fn repo_of(&self, domain: &str) -> Option<String> {
        let domain = domain.trim();
        self.env
            .origins()
            .into_iter()
            .find(|(name, _)| name == domain)
            .map(|(_, repo)| repo)
    }

    /// The fresh snapshot of `key`, if there is one.
    fn fresh(&self, key: &str, now: Instant) -> Option<Arc<Snapshot>> {
        lock(&self.state)
            .entries
            .get(key)
            .filter(|entry| now < entry.fetched_at + FRESH_FOR)
            .map(|entry| entry.snapshot.clone())
    }

    /// The snapshot inside the stale window, if there is one; one past it
    /// is dropped.
    fn stale(state: &mut State, key: &str, now: Instant) -> Option<Arc<Snapshot>> {
        let entry = state.entries.get(key)?;
        if now < entry.fetched_at + FRESH_FOR + STALE_FOR {
            return Some(entry.snapshot.clone());
        }
        state.entries.remove(key);
        None
    }

    /// The snapshot to answer from for `repo`: the fresh one, else a new
    /// lookup, else the stale one inside the window, else `None`.
    async fn snapshot(&self, source: &dyn GithubSource, repo: &str) -> Option<Arc<Snapshot>> {
        let key = repo_key(repo);
        if let Some(snapshot) = self.fresh(&key, Instant::now()) {
            return Some(snapshot);
        }
        let gate = lock(&self.fetching).entry(key.clone()).or_default().clone();
        let _held = gate.lock().await;
        // Another caller may have looked it up while this one waited.
        let now = Instant::now();
        if let Some(snapshot) = self.fresh(&key, now) {
            return Some(snapshot);
        }
        {
            let mut state = lock(&self.state);
            if state.not_installed.contains_key(&key) {
                return None;
            }
            if let Some(failing) = state.failing.get(&key).copied()
                && now < failing.last + RETRY_AFTER_FAILURE
            {
                return Self::stale(&mut state, &key, now);
            }
        }
        match fetch(source, repo).await {
            Ok(snapshot) => {
                let snapshot = Arc::new(snapshot);
                {
                    let mut state = lock(&self.state);
                    state.entries.insert(
                        key.clone(),
                        Entry {
                            snapshot: snapshot.clone(),
                            fetched_at: now,
                        },
                    );
                    if state.failing.remove(&key).is_some() {
                        tracing::info!("GitHub answers again who may use {repo}");
                    }
                }
                self.refresh_logins(&snapshot).await;
                Some(snapshot)
            }
            Err(error) => self.failed(repo, &key, &error, now),
        }
    }

    /// Records a failed lookup of `repo` and answers what is left.
    fn failed(
        &self,
        repo: &str,
        key: &str,
        error: &RemoteError,
        now: Instant,
    ) -> Option<Arc<Snapshot>> {
        let mut state = lock(&self.state);
        match Outcome::of(error) {
            Outcome::NotInstalled => {
                tracing::info!(
                    "the GitHub App is not installed on {repo}, so its domain's access is decided by Crystalline alone"
                );
                state.entries.remove(key);
                state.failing.remove(key);
                state.not_installed.insert(key.to_string(), now);
                None
            }
            Outcome::Gone => {
                tracing::warn!(
                    "GitHub does not show {repo} to the App ({error}), so nobody may use its domain until it does"
                );
                state.entries.remove(key);
                Self::mark_failing(&mut state, key, now);
                None
            }
            Outcome::Failed => {
                Self::mark_failing(&mut state, key, now);
                let stale = Self::stale(&mut state, key, now);
                let what = match error {
                    RemoteError::AuthExpired => {
                        "GitHub refused the App's installation token twice".to_string()
                    }
                    other => other.to_string(),
                };
                if stale.is_some() {
                    tracing::warn!(
                        "GitHub could not say who may use {repo} ({what}); the last answer stands for up to an hour"
                    );
                } else {
                    tracing::warn!(
                        "GitHub could not say who may use {repo} ({what}), and no recent answer is known, so nobody may use its domain until it can"
                    );
                }
                stale
            }
        }
    }

    fn mark_failing(state: &mut State, key: &str, now: Instant) {
        let since = state
            .failing
            .get(key)
            .map_or_else(Utc::now, |failing| failing.since);
        state
            .failing
            .insert(key.to_string(), Failing { since, last: now });
    }

    /// Stores the login the listing shows for each linked id whose stored
    /// login differs (the GitHub account was renamed). The stored login is
    /// only a cache: access follows the id.
    async fn refresh_logins(&self, snapshot: &Snapshot) {
        let Some(access) = self.env.access() else {
            return;
        };
        let links = match access.github_links().await {
            Ok(links) => links,
            Err(e) => {
                tracing::debug!("the GitHub links could not be read to refresh logins: {e:#}");
                return;
            }
        };
        let GovernorApp::Ready { host, .. } = self.env.app() else {
            return;
        };
        for link in links.iter().filter(|link| on_host(link, &host)) {
            let Some((login, _)) = snapshot.people.get(&link.github_id) else {
                continue;
            };
            if *login == link.login {
                continue;
            }
            match access.set_github_login(&link.account, login).await {
                Ok(_) => tracing::info!(
                    "the GitHub login of {} is now {login} (it was {})",
                    link.account,
                    link.login
                ),
                Err(e) => tracing::debug!(
                    "the new GitHub login of {} could not be stored: {e:#}",
                    link.account
                ),
            }
        }
    }

    /// Asks again, in the background, about every "not installed" answer
    /// older than [`FRESH_FOR`], so an App installed later governs without a
    /// restart even when no webhook says so.
    fn recheck_not_installed(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let now = Instant::now();
        let due: Vec<String> = {
            let mut state = lock(&self.state);
            let due: Vec<String> = state
                .not_installed
                .iter()
                .filter(|(_, at)| now >= **at + FRESH_FOR)
                .map(|(repo, _)| repo.clone())
                .filter(|repo| !state.rechecking.contains(repo))
                .collect();
            state.rechecking.extend(due.iter().cloned());
            due
        };
        if due.is_empty() {
            return;
        }
        let Some(me) = self.me.upgrade() else {
            return;
        };
        runtime.spawn(async move {
            let source = match me.env.app() {
                GovernorApp::Ready { source, .. } => Some(source),
                GovernorApp::Absent | GovernorApp::Unusable => None,
            };
            for repo in due {
                let answer = match &source {
                    Some(source) => Some(source.installation_for(&repo).await),
                    None => None,
                };
                let mut state = lock(&me.state);
                state.rechecking.remove(&repo);
                match answer {
                    Some(Ok(_)) => {
                        tracing::info!(
                            "the GitHub App is now installed on {repo}, so its domain's access follows GitHub"
                        );
                        state.not_installed.remove(&repo);
                    }
                    Some(Err(RemoteError::AppNotInstalled { .. })) => {
                        state.not_installed.insert(repo, Instant::now());
                    }
                    // Not known either way: the last answer stands and is
                    // asked about again on the next question.
                    Some(Err(_)) | None => {}
                }
            }
        });
    }
}

/// One snapshot of `repo`: its collaborators and whether it is public.
async fn fetch(source: &dyn GithubSource, repo: &str) -> Result<Snapshot, RemoteError> {
    let people = source.collaborators(repo).await?;
    let public = source.repository_public(repo).await?;
    Ok(Snapshot {
        public,
        people: people
            .into_iter()
            .map(|person| (person.id, (person.login, person.role)))
            .collect(),
    })
}

#[async_trait]
impl KnowledgeGovernor for GithubGovernor {
    fn governed_domains(&self) -> HashSet<String> {
        if matches!(self.env.app(), GovernorApp::Absent) {
            return HashSet::new();
        }
        self.recheck_not_installed();
        let state = lock(&self.state);
        self.env
            .origins()
            .into_iter()
            .filter(|(_, repo)| !state.not_installed.contains_key(&repo_key(repo)))
            .map(|(name, _)| name)
            .collect()
    }

    async fn knowledge(&self, account: &str, domain: &str) -> GithubKnowledge {
        let Some(access) = self.env.access() else {
            return GithubKnowledge::Unlinked;
        };
        let link = match access.github_link_of(account).await {
            Ok(Some(link)) => link,
            Ok(None) => return GithubKnowledge::Unlinked,
            Err(e) => {
                tracing::warn!("the GitHub link of {account} could not be read: {e:#}");
                return GithubKnowledge::Level(MemberLevel::None);
            }
        };
        let GovernorApp::Ready { source, host } = self.env.app() else {
            return GithubKnowledge::Level(MemberLevel::None);
        };
        if !on_host(&link, &host) {
            return GithubKnowledge::Unlinked;
        }
        match access.account_enabled(account).await {
            Ok(true) => {}
            Ok(false) => return GithubKnowledge::Level(MemberLevel::None),
            Err(e) => {
                tracing::warn!("the account {account} could not be read: {e:#}");
                return GithubKnowledge::Level(MemberLevel::None);
            }
        }
        let Some(repo) = self.repo_of(domain) else {
            return GithubKnowledge::Level(MemberLevel::None);
        };
        match self.snapshot(source.as_ref(), &repo).await {
            Some(snapshot) => GithubKnowledge::Level(snapshot.level_of(link.github_id)),
            None => GithubKnowledge::Level(MemberLevel::None),
        }
    }

    async fn repository_public(&self, domain: &str) -> bool {
        let Some(repo) = self.repo_of(domain) else {
            return false;
        };
        let GovernorApp::Ready { source, .. } = self.env.app() else {
            return false;
        };
        self.snapshot(source.as_ref(), &repo)
            .await
            .is_some_and(|snapshot| snapshot.public)
    }

    async fn readers(&self, domain: &str) -> Option<HashSet<String>> {
        let repo = self.repo_of(domain)?;
        let access = self.env.access()?;
        let GovernorApp::Ready { source, host } = self.env.app() else {
            return None;
        };
        let snapshot = self.snapshot(source.as_ref(), &repo).await?;
        let links = match access.github_links().await {
            Ok(links) => links,
            Err(e) => {
                tracing::warn!("the GitHub links could not be read: {e:#}");
                return None;
            }
        };
        let enabled = match access.enabled_accounts().await {
            Ok(enabled) => enabled,
            Err(e) => {
                tracing::warn!("the accounts could not be read: {e:#}");
                return None;
            }
        };
        Some(
            links
                .into_iter()
                .filter(|link| on_host(link, &host) && enabled.contains(&link.account))
                .filter(|link| snapshot.level_of(link.github_id) != MemberLevel::None)
                .map(|link| link.account)
                .collect(),
        )
    }
}

/// The governor's view of an engine. A [`Weak`] handle: the engine holds
/// the governor, and every [`DomainAccess`] the engine holds holds it too.
struct EngineEnv {
    engine: Weak<super::Engine>,
}

impl GovernorEnv for EngineEnv {
    fn app(&self) -> GovernorApp {
        let Some(engine) = self.engine.upgrade() else {
            return GovernorApp::Unusable;
        };
        match engine.github_app_slot() {
            super::github_app::AppSlot::Absent => GovernorApp::Absent,
            super::github_app::AppSlot::Ready(router) => GovernorApp::Ready {
                host: router.app().credentials().host.clone(),
                source: router.app().clone(),
            },
            // Configured but not usable (or, never returned here, not
            // looked for): governed, and nobody gets anything.
            super::github_app::AppSlot::Refused(_) | super::github_app::AppSlot::Unloaded => {
                GovernorApp::Unusable
            }
        }
    }

    fn origins(&self) -> Vec<(String, String)> {
        let Some(engine) = self.engine.upgrade() else {
            return Vec::new();
        };
        let config = engine.config.read().unwrap();
        let mut origins: Vec<(String, String)> = config
            .domains
            .iter()
            .filter_map(|(name, entry)| Some((name.clone(), entry.origin.as_ref()?.repo.clone())))
            .collect();
        for (name, entry) in engine.discovered_domains.read().unwrap().iter() {
            if config.domains.contains_key(name) {
                continue;
            }
            if let Some(origin) = &entry.origin {
                origins.push((name.clone(), origin.repo.clone()));
            }
        }
        origins
    }

    fn access(&self) -> Option<Arc<DomainAccess>> {
        self.engine.upgrade()?.domain_access.get().cloned()
    }
}

impl super::Engine {
    /// The GitHub governor of this engine, built on first use. Every caller
    /// gets the same one, so every [`DomainAccess`] answers from one cache.
    pub fn github_governor(self: &Arc<Self>) -> Arc<GithubGovernor> {
        self.github_governor
            .get_or_init(|| {
                GithubGovernor::new(Arc::new(EngineEnv {
                    engine: Arc::downgrade(self),
                }))
            })
            .clone()
    }

    /// The governor every [`DomainAccess`] this engine is served through is
    /// built with ([`DomainAccess::with_governor`]): on the domains connected
    /// to GitHub through the GitHub App, access follows the person's GitHub
    /// role. Without an App it governs nothing.
    pub fn knowledge_governor(self: &Arc<Self>) -> Arc<dyn KnowledgeGovernor> {
        self.github_governor()
    }

    /// Since when GitHub has not answered who may use a connected domain
    /// (see [`GithubGovernor::degraded_since`]), `None` while it answers or
    /// before anything was asked.
    pub fn github_access_degraded(&self) -> Option<DateTime<Utc>> {
        self.github_governor.get()?.degraded_since()
    }

    /// Whether GitHub decides who reads and writes `domain` in the resolver
    /// this engine serves through. `false` with no resolver installed or no
    /// governor in it.
    pub fn github_governs(&self, domain: &str) -> bool {
        self.domain_access
            .get()
            .is_some_and(|access| access.governs(domain))
    }

    /// Whether `scope` is the MCP open tier: an anonymous caller on an
    /// instance with `auth.mcp` off.
    pub fn is_open_tier(&self, scope: &Scope) -> bool {
        matches!(scope, Scope::Anonymous) && !self.auth_mcp()
    }

    /// Whether the open tier's carve-out holds for `scope` on `domain`: the
    /// open tier writes and curates wherever it reads, except on a domain
    /// GitHub governs, where it is decided like everybody else (as
    /// `github.agent_identity` when that is set, see [`Engine::scope_on`]).
    pub fn open_tier_carve_out(&self, scope: &Scope, domain: &str) -> bool {
        self.is_open_tier(scope) && !self.github_governs(domain)
    }

    /// The account the open tier acts as on the domains GitHub governs:
    /// `github.agent_identity`, when `scope` is the open tier and it is set.
    pub(crate) fn open_tier_agent(&self, scope: &Scope) -> Option<String> {
        if !self.is_open_tier(scope) {
            return None;
        }
        self.config
            .read()
            .unwrap()
            .github_agent_identity()
            .map(str::to_string)
    }

    /// The scope a question about `domain` is decided for: `scope` itself,
    /// except the open tier on a domain GitHub governs, which acts as
    /// `github.agent_identity` (as `Scope::User` without the admin flag)
    /// when that is set. Without it the open tier stays anonymous there.
    pub fn scope_on(&self, scope: &Scope, domain: &str) -> Scope {
        match self.open_tier_agent(scope) {
            Some(account) if self.github_governs(domain) => Scope::User {
                account,
                admin: false,
            },
            _ => scope.clone(),
        }
    }

    /// What `scope` sees across every domain: [`DomainAccess::visibility`],
    /// or for the open tier acting as `github.agent_identity`
    /// [`DomainAccess::open_tier_visibility`].
    pub(crate) async fn visibility_in(
        &self,
        access: &DomainAccess,
        scope: &Scope,
    ) -> anyhow::Result<DomainVisibility> {
        match self.open_tier_agent(scope) {
            Some(agent) => access.open_tier_visibility(&agent).await,
            None => access.visibility(scope).await,
        }
    }

    /// Forget what the governor learned about `what`, when it has learned
    /// anything.
    pub fn invalidate_github_access(&self, what: Invalidate) {
        if let Some(governor) = self.github_governor.get() {
            governor.invalidate(what);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_store::{AuthStore, Role};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const REPO: &str = "acme/handbook";
    const DOMAIN: &str = "handbook";
    const MIN: Duration = Duration::from_secs(60);

    /// One repository as the fake GitHub holds it.
    #[derive(Default)]
    struct FakeRepo {
        people: Vec<Collaborator>,
        public: bool,
        /// Every call on the repository fails with this, while set.
        down: Option<fn() -> RemoteError>,
        not_installed: bool,
    }

    /// GitHub, answering from tables and counting the collaborator listings
    /// and installation lookups it was asked for.
    #[derive(Default)]
    struct FakeGithub {
        repos: Mutex<HashMap<String, FakeRepo>>,
        listings: AtomicUsize,
        installation_lookups: AtomicUsize,
    }

    impl FakeGithub {
        fn with(&self, repo: &str, change: impl FnOnce(&mut FakeRepo)) {
            change(lock(&self.repos).entry(repo_key(repo)).or_default());
        }

        fn grant(&self, repo: &str, id: u64, login: &str, role: RepoRole) {
            self.with(repo, |r| {
                r.people.retain(|p| p.id != id);
                r.people.push(Collaborator {
                    id,
                    login: login.to_string(),
                    role,
                });
            });
        }

        fn listings(&self) -> usize {
            self.listings.load(Ordering::SeqCst)
        }

        fn refusal(&self, repo: &str) -> Option<RemoteError> {
            let repos = lock(&self.repos);
            let r = repos.get(&repo_key(repo))?;
            if r.not_installed {
                return Some(RemoteError::AppNotInstalled {
                    repo: repo.to_string(),
                    install_url: "https://github.com/apps/x/installations/new".to_string(),
                });
            }
            r.down.map(|down| down())
        }
    }

    #[async_trait]
    impl GithubSource for FakeGithub {
        async fn collaborators(&self, repo: &str) -> Result<Vec<Collaborator>, RemoteError> {
            self.listings.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.refusal(repo) {
                return Err(e);
            }
            Ok(lock(&self.repos)
                .get(&repo_key(repo))
                .map(|r| r.people.clone())
                .unwrap_or_default())
        }

        async fn repository_public(&self, repo: &str) -> Result<bool, RemoteError> {
            if let Some(e) = self.refusal(repo) {
                return Err(e);
            }
            Ok(lock(&self.repos)
                .get(&repo_key(repo))
                .is_some_and(|r| r.public))
        }

        async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError> {
            self.installation_lookups.fetch_add(1, Ordering::SeqCst);
            match self.refusal(repo) {
                Some(e) => Err(e),
                None => Ok(7),
            }
        }
    }

    /// The engine's side, from fixed values.
    struct FakeEnv {
        app: Mutex<GovernorApp>,
        origins: Mutex<Vec<(String, String)>>,
        access: Option<Arc<DomainAccess>>,
    }

    impl GovernorEnv for FakeEnv {
        fn app(&self) -> GovernorApp {
            lock(&self.app).clone()
        }
        fn origins(&self) -> Vec<(String, String)> {
            lock(&self.origins).clone()
        }
        fn access(&self) -> Option<Arc<DomainAccess>> {
            self.access.clone()
        }
    }

    struct Rig {
        github: Arc<FakeGithub>,
        env: Arc<FakeEnv>,
        governor: Arc<GithubGovernor>,
        auth: Arc<AuthStore>,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        async fn knowledge(&self, account: &str) -> GithubKnowledge {
            self.governor.knowledge(account, DOMAIN).await
        }
    }

    /// A governor over one domain `handbook` on `acme/handbook`, with the
    /// account `alice` (instance editor) linked to GitHub id 1.
    async fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let auth = Arc::new(
            AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        auth.add_user("alice", "Alice", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.link_github("alice", "github.com", 1, "alice-gh")
            .await
            .unwrap();
        let github = Arc::new(FakeGithub::default());
        github.with(REPO, |_| {});
        let env = Arc::new(FakeEnv {
            app: Mutex::new(GovernorApp::Ready {
                source: github.clone(),
                host: "github.com".to_string(),
            }),
            origins: Mutex::new(vec![(DOMAIN.to_string(), REPO.to_string())]),
            access: Some(Arc::new(DomainAccess::new(auth.clone()))),
        });
        let governor = GithubGovernor::new(env.clone());
        Rig {
            github,
            env,
            governor,
            auth,
            _dir: dir,
        }
    }

    fn level(level: MemberLevel) -> GithubKnowledge {
        GithubKnowledge::Level(level)
    }

    #[test]
    fn the_role_table_maps_every_github_role() {
        assert_eq!(level_for(RepoRole::Admin), MemberLevel::Manager);
        assert_eq!(level_for(RepoRole::Maintain), MemberLevel::Manager);
        assert_eq!(level_for(RepoRole::Write), MemberLevel::Editor);
        assert_eq!(level_for(RepoRole::Triage), MemberLevel::Viewer);
        assert_eq!(level_for(RepoRole::Read), MemberLevel::Viewer);
        assert_eq!(level_for(RepoRole::None), MemberLevel::None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_role_is_cached_for_five_minutes() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        tokio::time::advance(4 * MIN + Duration::from_secs(59)).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.github.listings(), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.github.listings(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_none_answer_is_cached_too() {
        let rig = rig().await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        tokio::time::advance(4 * MIN + Duration::from_secs(59)).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.github.listings(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn github_down_serves_the_last_role_for_one_hour_past_expiry() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        tokio::time::advance(5 * MIN + 59 * MIN).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        tokio::time::advance(2 * MIN).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
    }

    #[tokio::test(start_paused = true)]
    async fn failures_to_learn_serve_the_last_role_inside_the_window() {
        let failures: [fn() -> RemoteError; 6] = [
            || RemoteError::RateLimited { reset: None },
            || RemoteError::Api {
                status: 502,
                message: "bad gateway".to_string(),
            },
            || RemoteError::Api {
                status: 403,
                message: "forbidden".to_string(),
            },
            || RemoteError::Api {
                status: 301,
                message: "moved twice".to_string(),
            },
            || RemoteError::AuthExpired,
            || RemoteError::AppCredential("the key does not parse".to_string()),
        ];
        for failure in failures {
            let rig = rig().await;
            rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
            assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
            rig.github.with(REPO, |r| r.down = Some(failure));
            tokio::time::advance(30 * MIN).await;
            assert_eq!(
                rig.knowledge("alice").await,
                level(MemberLevel::Editor),
                "{}",
                failure()
            );
            assert!(rig.governor.degraded_since().is_some(), "{}", failure());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_repository_github_no_longer_shows_is_closed_at_once() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        rig.github.with(REPO, |r| {
            r.down = Some(|| RemoteError::RepoNotFound {
                repo: REPO.to_string(),
            })
        });
        tokio::time::advance(6 * MIN).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert!(rig.governor.degraded_since().is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn degraded_is_reported_and_clears_on_success() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.governor.degraded_since(), None);
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        tokio::time::advance(6 * MIN).await;
        let before = Utc::now();
        rig.knowledge("alice").await;
        let since = rig.governor.degraded_since().expect("degraded");
        assert!(since >= before - chrono::Duration::seconds(5));
        // A second failure keeps the first one's time.
        tokio::time::advance(MIN).await;
        rig.knowledge("alice").await;
        assert_eq!(rig.governor.degraded_since(), Some(since));
        rig.github.with(REPO, |r| r.down = None);
        tokio::time::advance(MIN).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.governor.degraded_since(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_lookup_is_not_repeated_for_every_question() {
        let rig = rig().await;
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        rig.knowledge("alice").await;
        rig.knowledge("alice").await;
        assert_eq!(rig.github.listings(), 1);
        tokio::time::advance(RETRY_AFTER_FAILURE).await;
        rig.knowledge("alice").await;
        assert_eq!(rig.github.listings(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_domain_on_a_repo_without_the_app_is_not_governed() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        assert!(rig.governor.governed_domains().contains(DOMAIN));
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert!(!rig.governor.governed_domains().contains(DOMAIN));
        // Not installed is an answer, not an outage.
        assert_eq!(rig.governor.degraded_since(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_installed_later_governs_without_a_restart() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.knowledge("alice").await;
        assert!(!rig.governor.governed_domains().contains(DOMAIN));
        rig.github.with(REPO, |r| r.not_installed = false);
        tokio::time::advance(FRESH_FOR).await;
        // The first question past the expiry asks again in the background.
        assert!(!rig.governor.governed_domains().contains(DOMAIN));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(rig.github.installation_lookups.load(Ordering::SeqCst), 1);
        assert!(rig.governor.governed_domains().contains(DOMAIN));
    }

    #[tokio::test(start_paused = true)]
    async fn a_domain_whose_installation_is_unknown_is_closed_not_open() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        assert!(rig.governor.governed_domains().contains(DOMAIN));
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.governor.readers(DOMAIN).await, None);
        assert!(!rig.governor.repository_public(DOMAIN).await);
        assert_eq!(rig.governor.repository_readers(DOMAIN).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn without_an_app_nothing_is_governed_and_an_unusable_one_grants_nothing() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        *lock(&rig.env.app) = GovernorApp::Absent;
        assert!(rig.governor.governed_domains().is_empty());
        *lock(&rig.env.app) = GovernorApp::Unusable;
        assert!(rig.governor.governed_domains().contains(DOMAIN));
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.governor.readers(DOMAIN).await, None);
        assert_eq!(rig.github.listings(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn readers_come_from_one_cached_collaborators_listing() {
        let rig = rig().await;
        rig.auth
            .add_user("bob", "Bob", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        rig.auth
            .link_github("bob", "github.com", 2, "bob")
            .await
            .unwrap();
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.github.grant(REPO, 2, "bob", RepoRole::Read);
        let both: HashSet<String> = ["alice", "bob"].map(String::from).into();
        assert_eq!(rig.governor.readers(DOMAIN).await, Some(both.clone()));
        tokio::time::advance(4 * MIN).await;
        assert_eq!(rig.governor.readers(DOMAIN).await, Some(both));
        assert_eq!(rig.knowledge("bob").await, level(MemberLevel::Viewer));
        assert_eq!(rig.github.listings(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn readers_are_none_past_the_stale_window() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Read);
        assert!(rig.governor.readers(DOMAIN).await.is_some());
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        tokio::time::advance(5 * MIN + 59 * MIN).await;
        assert!(rig.governor.readers(DOMAIN).await.is_some());
        assert!(rig.governor.repository_readers(DOMAIN).await.is_some());
        tokio::time::advance(2 * MIN).await;
        assert_eq!(rig.governor.readers(DOMAIN).await, None);
        assert_eq!(rig.governor.repository_readers(DOMAIN).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn repository_readers_name_the_github_ids_with_read_or_more() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.github.grant(REPO, 5, "triager", RepoRole::Triage);
        rig.github.grant(REPO, 6, "nobody", RepoRole::None);
        rig.github.with(REPO, |r| r.public = true);
        assert_eq!(
            rig.governor.repository_readers(DOMAIN).await,
            Some(RepoReaders {
                public: true,
                github_ids: [1, 5].into(),
            })
        );
        assert!(rig.governor.repository_public(DOMAIN).await);
        assert_eq!(rig.github.listings(), 1);
    }

    /// An account for the agreement test: its name, its link (host and
    /// GitHub id) and its role on the repository.
    type CastMember = (&'static str, Option<(&'static str, u64)>, Option<RepoRole>);

    /// `readers` is exactly the accounts whose `knowledge` is viewer or
    /// more, in the store's own spelling, on a private and on a public
    /// repository.
    #[tokio::test(start_paused = true)]
    async fn readers_and_knowledge_agree_for_every_account() {
        for public in [false, true] {
            let rig = rig().await;
            // alice (id 1): write. Then one account per case.
            let cast: [CastMember; 8] = [
                ("maint", Some(("github.com", 2)), Some(RepoRole::Maintain)),
                ("reader", Some(("github.com", 3)), Some(RepoRole::Read)),
                ("triager", Some(("github.com", 4)), Some(RepoRole::Triage)),
                ("listed-none", Some(("github.com", 5)), Some(RepoRole::None)),
                ("outsider", Some(("github.com", 6)), None),
                (
                    "elsewhere",
                    Some(("ghe.example.com", 7)),
                    Some(RepoRole::Write),
                ),
                ("unlinked", None, None),
                ("gone", Some(("github.com", 8)), Some(RepoRole::Admin)),
            ];
            rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
            rig.github.with(REPO, |r| r.public = public);
            for (name, link, role) in cast {
                rig.auth
                    .add_user(name, name, None, Role::Viewer, "pw12345678")
                    .await
                    .unwrap();
                if let Some((host, id)) = link {
                    rig.auth.link_github(name, host, id, name).await.unwrap();
                    if let Some(role) = role {
                        rig.github.grant(REPO, id, name, role);
                    }
                }
            }
            rig.auth.set_disabled("gone", true).await.unwrap();
            let readers = rig.governor.readers(DOMAIN).await.expect("known");
            let mut accounts = vec!["alice".to_string()];
            accounts.extend(cast.iter().map(|(name, _, _)| name.to_string()));
            let mut expected = HashSet::new();
            for account in &accounts {
                let knowledge = rig.knowledge(account).await;
                if matches!(
                    knowledge,
                    GithubKnowledge::Level(
                        MemberLevel::Viewer | MemberLevel::Editor | MemberLevel::Manager
                    )
                ) {
                    expected.insert(account.clone());
                }
                // Asked in another spelling, the answer is the same.
                assert_eq!(
                    rig.knowledge(&account.to_uppercase()).await,
                    knowledge,
                    "{account}"
                );
            }
            assert_eq!(readers, expected, "public: {public}");
            let mut must: HashSet<String> = ["alice", "maint", "reader", "triager"]
                .map(String::from)
                .into();
            if public {
                must.extend(["listed-none", "outsider"].map(String::from));
            }
            assert_eq!(readers, must, "public: {public}");
            assert_eq!(rig.knowledge("unlinked").await, GithubKnowledge::Unlinked);
            assert_eq!(rig.knowledge("elsewhere").await, GithubKnowledge::Unlinked);
            assert_eq!(rig.knowledge("gone").await, level(MemberLevel::None));
            assert_eq!(rig.github.listings(), 1, "public: {public}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_unlinked_account_is_unlinked_at_once_whatever_the_cache_says() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        rig.auth.unlink_github("alice").await.unwrap();
        assert_eq!(rig.knowledge("alice").await, GithubKnowledge::Unlinked);
        assert_eq!(rig.governor.readers(DOMAIN).await, Some(HashSet::new()));
    }

    #[tokio::test(start_paused = true)]
    async fn invalidate_drops_the_stale_fallback_too() {
        for what in [
            Invalidate::Repo("ACME/Handbook".to_string()),
            Invalidate::User(1),
            Invalidate::Installation(7),
            Invalidate::All,
        ] {
            let rig = rig().await;
            rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
            assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
            rig.github
                .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
            rig.governor.invalidate(what.clone());
            assert_eq!(
                rig.knowledge("alice").await,
                level(MemberLevel::None),
                "{what:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn invalidate_asks_again_about_an_installation() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.knowledge("alice").await;
        assert!(!rig.governor.governed_domains().contains(DOMAIN));
        rig.governor.invalidate(Invalidate::Installation(7));
        assert!(rig.governor.governed_domains().contains(DOMAIN));
    }

    #[tokio::test(start_paused = true)]
    async fn a_renamed_github_account_keeps_its_access_and_its_new_login_is_stored() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-renamed", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        let link = rig.auth.github_link_of("alice").await.unwrap().unwrap();
        assert_eq!(link.login, "alice-renamed");
    }
}
