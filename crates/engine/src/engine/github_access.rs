//! Who may read and write a domain connected to GitHub through the GitHub
//! App: [`GithubGovernor`], the [`KnowledgeGovernor`] the engine installs
//! into every [`DomainAccess`] it serves through.
//!
//! **What is governed.** A domain is governed when an App is configured
//! (usable or not) and the domain has an `origin`, decided from the
//! configuration alone, so a domain is governed from the first request on,
//! with no access for anybody until the first lookup lands. Nothing takes
//! it out again while the App is configured: when the App is not installed
//! on the repository, or GitHub does not show the repository to the App,
//! the domain is **closed** ([`KnowledgeGovernor::closed_domains`]): only
//! the machine owner and instance admins get in, and no member, visibility
//! or open-tier rule applies. A closed domain is asked about again in the
//! background every five minutes, and at once after an invalidation, so
//! installing the App opens it without a restart. An App that is
//! configured but cannot be used governs every origin domain and grants
//! nothing on any of them.
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
//! [`Outcome::of`]: the App not being installed and a repository GitHub no
//! longer shows are answers that close the domain, and everything else is
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

/// How long one GitHub request of a lookup (one page of the collaborator
/// listing, the visibility, an installation lookup) may take. One that
/// takes longer is a failure to learn, like GitHub being down.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a question waits for a lookup. Past it the question is answered
/// from what is known (the stale snapshot inside the window, else no
/// access), and the lookup goes on in the background and stores its answer
/// for later questions, so a large repository whose listing takes many
/// pages is not closed for good.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

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

/// A domain closed because the App is not installed on its repository (or
/// GitHub does not show the repository to the App), as the doctor and the
/// admin banner name it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ClosedDomain {
    /// The domain.
    pub domain: String,
    /// Its repository, `owner/name`.
    pub repo: String,
    /// The sentence a person acts on.
    pub reason: String,
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
    /// One page of the people with access to `repo`, and the next page,
    /// see [`GithubApp::collaborators_page`].
    async fn collaborators_page(
        &self,
        repo: &str,
        page: Option<&str>,
    ) -> Result<(Vec<Collaborator>, Option<String>), RemoteError>;
    /// Whether `repo` is public, see [`GithubApp::repository_public`].
    async fn repository_public(&self, repo: &str) -> Result<bool, RemoteError>;
    /// The installation that covers `repo`, see [`GithubApp::installation_for`].
    async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError>;
    /// Forget which installation covers `repo`, see [`GithubApp::evict_repo`].
    fn evict_repo(&self, repo: &str);
}

#[async_trait]
impl GithubSource for GithubApp {
    async fn collaborators_page(
        &self,
        repo: &str,
        page: Option<&str>,
    ) -> Result<(Vec<Collaborator>, Option<String>), RemoteError> {
        GithubApp::collaborators_page(self, repo, page).await
    }

    async fn repository_public(&self, repo: &str) -> Result<bool, RemoteError> {
        GithubApp::repository_public(self, repo).await
    }

    async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError> {
        GithubApp::installation_for(self, repo).await
    }

    fn evict_repo(&self, repo: &str) {
        GithubApp::evict_repo(self, repo);
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
    /// Repositories GitHub said the App is not installed on, and when to
    /// ask again (in the background, see `GithubGovernor::recheck_closed`).
    /// Their domains are closed.
    not_installed: HashMap<String, Instant>,
    /// Repositories whose "not installed" answer is being asked again.
    rechecking: HashSet<String>,
    /// Repositories whose last lookup failed: when the failures began, and
    /// when the last one was.
    failing: HashMap<String, Failing>,
    /// Bumped by every invalidation of one repository.
    generations: HashMap<String, u64>,
    /// Bumped by every invalidation of more than one repository.
    global: u64,
}

impl State {
    /// The invalidations `key` has seen so far. A lookup takes this before
    /// it asks GitHub and stores its answer only when it is unchanged after,
    /// so an invalidation that lands while GitHub is asked is never undone
    /// by the answer to the question before it.
    fn generation(&self, key: &str) -> Generation {
        Generation {
            repo: self.generations.get(key).copied().unwrap_or_default(),
            global: self.global,
        }
    }
}

/// See [`State::generation`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Generation {
    repo: u64,
    global: u64,
}

/// An ongoing failure to learn about one repository.
#[derive(Clone, Copy)]
struct Failing {
    /// The first failure since the last good lookup, as wall time.
    since: DateTime<Utc>,
    /// When the last failed lookup failed, for [`RETRY_AFTER_FAILURE`];
    /// `None` once an invalidation asks for a new lookup at once.
    last: Option<Instant>,
    /// Whether the last failure was [`Outcome::Gone`]: no stale answer is
    /// served, the domain is closed.
    gone: bool,
}

/// Why one lookup gave no snapshot.
#[derive(Debug)]
enum FetchError {
    /// What GitHub (or the App client) answered.
    Remote(RemoteError),
    /// One request got no answer within [`REQUEST_TIMEOUT`].
    TimedOut,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Remote(RemoteError::AuthExpired) => {
                f.write_str("GitHub refused the App's installation token twice")
            }
            FetchError::Remote(e) => e.fmt(f),
            FetchError::TimedOut => write!(
                f,
                "a request to GitHub timed out after {} seconds",
                REQUEST_TIMEOUT.as_secs()
            ),
        }
    }
}

/// What one failed lookup means, decided by [`Outcome::of`].
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The App is not installed on the repository: the domain is closed.
    /// An answer, not a failure (nothing is reported degraded).
    NotInstalled,
    /// GitHub says the repository is not there (deleted, or not visible to
    /// the App): the domain is closed at once, no stale answer served.
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
        self.connected_failures()
            .into_iter()
            .map(|(_, failing)| failing.since)
            .min()
    }

    /// Since when a repository's lookups have been failing while its last
    /// answer may still serve (GitHub down, a rate limit and the like), or
    /// `None`.
    pub fn stale_since(&self) -> Option<DateTime<Utc>> {
        self.connected_failures()
            .into_iter()
            .filter(|(_, failing)| !failing.gone)
            .map(|(_, failing)| failing.since)
            .min()
    }

    /// The closed domains with their repository and the reason, sorted by
    /// domain: the App is not installed on the repository, or GitHub does
    /// not show it to the App. Only the machine owner and instance admins
    /// get into them. For the doctor and the admin banner.
    pub fn closed(&self) -> Vec<ClosedDomain> {
        let closed = self.closed_keys();
        let mut out: Vec<ClosedDomain> = self
            .env
            .origins()
            .into_iter()
            .filter(|(_, repo)| closed.contains(&repo_key(repo)))
            .map(|(domain, repo)| ClosedDomain {
                reason: format!(
                    "the GitHub App is not installed on {repo}: install it there or disconnect the domain"
                ),
                domain,
                repo,
            })
            .collect();
        out.sort_by(|a, b| a.domain.cmp(&b.domain));
        out
    }

    /// The repositories whose domains are closed, by cache key.
    fn closed_keys(&self) -> HashSet<String> {
        let state = lock(&self.state);
        state
            .not_installed
            .keys()
            .cloned()
            .chain(
                state
                    .failing
                    .iter()
                    .filter(|(_, failing)| failing.gone)
                    .map(|(repo, _)| repo.clone()),
            )
            .collect()
    }

    /// The failures of repositories still connected to a domain.
    fn connected_failures(&self) -> Vec<(String, Failing)> {
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
            .map(|(repo, failing)| (repo.clone(), *failing))
            .collect()
    }

    /// Asks for the App once, so an unsettled slot starts loading in the
    /// background now rather than on the next request.
    pub fn preload_app(&self) {
        let _ = self.env.app();
    }

    /// Forget what `what` names, the stale fallback included.
    ///
    /// A lookup in flight when this lands answers its own question but is
    /// not stored, and the pause after a failed lookup ends, so the next
    /// question asks GitHub at once. A closed domain the invalidation names
    /// stays closed and is asked about again at once, in the background, so
    /// installing the App opens it without waiting five minutes. `All` also
    /// forgets every failure of an open domain, so nothing is reported
    /// degraded until a lookup fails again.
    pub fn invalidate(&self, what: Invalidate) {
        {
            let mut state = lock(&self.state);
            let now = Instant::now();
            match what {
                Invalidate::Repo(repo) => {
                    let key = repo_key(&repo);
                    *state.generations.entry(key.clone()).or_default() += 1;
                    state.entries.remove(&key);
                    if let Some(next) = state.not_installed.get_mut(&key) {
                        *next = now;
                    }
                    if let Some(failing) = state.failing.get_mut(&key) {
                        failing.last = None;
                    }
                }
                Invalidate::User(_) => {
                    state.global += 1;
                    state.entries.clear();
                    state.failing.values_mut().for_each(|f| f.last = None);
                }
                Invalidate::Installation(_) => {
                    state.global += 1;
                    state.entries.clear();
                    state
                        .not_installed
                        .values_mut()
                        .for_each(|next| *next = now);
                    state.failing.values_mut().for_each(|f| f.last = None);
                }
                Invalidate::All => {
                    state.global += 1;
                    state.entries.clear();
                    state
                        .not_installed
                        .values_mut()
                        .for_each(|next| *next = now);
                    state.failing.retain(|_, failing| failing.gone);
                    state.failing.values_mut().for_each(|f| f.last = None);
                }
            }
        }
        self.recheck_closed();
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
        let snapshot = self.snapshot(&source, &repo).await?;
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
    ///
    /// One lookup per repository at a time. A caller that finds one in
    /// flight answers from a snapshot inside the window when there is one,
    /// and waits for the lookup only when there is none. No caller waits
    /// longer than [`FETCH_TIMEOUT`]: the lookup runs as a task of its own
    /// and stores its answer whenever it comes.
    async fn snapshot(&self, source: &Arc<dyn GithubSource>, repo: &str) -> Option<Arc<Snapshot>> {
        let key = repo_key(repo);
        if let Some(snapshot) = self.fresh(&key, Instant::now()) {
            return Some(snapshot);
        }
        let gate = lock(&self.fetching).entry(key.clone()).or_default().clone();
        let held = match gate.clone().try_lock_owned() {
            Ok(held) => held,
            Err(_) => {
                if let Some(snapshot) = Self::stale(&mut lock(&self.state), &key, Instant::now()) {
                    return Some(snapshot);
                }
                match tokio::time::timeout(FETCH_TIMEOUT, gate.lock_owned()).await {
                    Ok(held) => held,
                    // Still looking: answer from what is known.
                    Err(_) => return Self::stale(&mut lock(&self.state), &key, Instant::now()),
                }
            }
        };
        // Another caller may have looked it up while this one waited.
        let started = Instant::now();
        if let Some(snapshot) = self.fresh(&key, started) {
            return Some(snapshot);
        }
        let generation = {
            let mut state = lock(&self.state);
            if state.not_installed.contains_key(&key) {
                return None;
            }
            if let Some(Failing {
                last: Some(last), ..
            }) = state.failing.get(&key).copied()
                && started < last + RETRY_AFTER_FAILURE
            {
                return Self::stale(&mut state, &key, started);
            }
            state.generation(&key)
        };
        let me = self.me.upgrade()?;
        let source = source.clone();
        let repo_owned = repo.to_string();
        let lookup = tokio::spawn(async move {
            // The gate stays held until the answer is stored.
            let _held = held;
            let result = fetch(source.as_ref(), &repo_owned).await;
            me.settle(source.as_ref(), &repo_owned, result, started, generation)
                .await
        });
        match tokio::time::timeout(FETCH_TIMEOUT, lookup).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(e)) => {
                tracing::warn!("the lookup of who may use {repo} stopped: {e}");
                None
            }
            // The lookup goes on and stores its answer; this question is
            // answered from what is known now.
            Err(_) => {
                tracing::info!(
                    "GitHub is slow to say who may use {repo}; answering from what is known while the lookup goes on"
                );
                Self::stale(&mut lock(&self.state), &key, Instant::now())
            }
        }
    }

    /// Stores what one lookup of `repo` found (only when no invalidation
    /// landed since `generation`) and answers it.
    async fn settle(
        &self,
        source: &dyn GithubSource,
        repo: &str,
        result: Result<Snapshot, FetchError>,
        started: Instant,
        generation: Generation,
    ) -> Option<Arc<Snapshot>> {
        let key = repo_key(repo);
        match result {
            Ok(snapshot) => {
                let snapshot = Arc::new(snapshot);
                {
                    let mut state = lock(&self.state);
                    if state.generation(&key) == generation {
                        state.entries.insert(
                            key.clone(),
                            Entry {
                                snapshot: snapshot.clone(),
                                fetched_at: started,
                            },
                        );
                    }
                    if state.failing.remove(&key).is_some() {
                        tracing::info!("GitHub answers again who may use {repo}");
                    }
                }
                self.refresh_logins(&snapshot).await;
                Some(snapshot)
            }
            // The failure is timed when it happened, not when it was asked.
            Err(error) => self.failed(source, repo, &key, &error, Instant::now(), generation),
        }
    }

    /// Records a failed lookup of `repo` and answers what is left. An
    /// invalidation since `generation` means the failure is about a
    /// question nobody asks any more: it is answered but not recorded.
    fn failed(
        &self,
        source: &dyn GithubSource,
        repo: &str,
        key: &str,
        error: &FetchError,
        now: Instant,
        generation: Generation,
    ) -> Option<Arc<Snapshot>> {
        let outcome = match error {
            FetchError::Remote(e) => Outcome::of(e),
            FetchError::TimedOut => Outcome::Failed,
        };
        let mut state = lock(&self.state);
        let current = state.generation(key) == generation;
        match outcome {
            Outcome::NotInstalled => {
                tracing::info!(
                    "the GitHub App is not installed on {repo}, so its domain is closed to all but instance admins until it is"
                );
                if current {
                    state.entries.remove(key);
                    state.failing.remove(key);
                    state.not_installed.insert(key.to_string(), now + FRESH_FOR);
                }
                None
            }
            Outcome::Gone => {
                tracing::warn!(
                    "GitHub does not show {repo} to the App ({error}), so nobody may use its domain until it does"
                );
                // Look the installation up again next time, as a restart
                // would.
                source.evict_repo(repo);
                if current {
                    state.entries.remove(key);
                    Self::mark_failing(&mut state, key, now, true);
                }
                None
            }
            Outcome::Failed => {
                if current {
                    Self::mark_failing(&mut state, key, now, false);
                }
                let stale = Self::stale(&mut state, key, now);
                if stale.is_some() {
                    tracing::warn!(
                        "GitHub could not say who may use {repo} ({error}); the last answer stands for up to an hour"
                    );
                } else {
                    tracing::warn!(
                        "GitHub could not say who may use {repo} ({error}), and no recent answer is known, so nobody may use its domain until it can"
                    );
                }
                stale
            }
        }
    }

    /// Records a failure. A closed repository stays closed through later
    /// failures to learn: only a good lookup opens it.
    fn mark_failing(state: &mut State, key: &str, now: Instant, gone: bool) {
        let earlier = state.failing.get(key).copied();
        let since = earlier.map_or_else(Utc::now, |failing| failing.since);
        let gone = gone || earlier.is_some_and(|failing| failing.gone);
        state.failing.insert(
            key.to_string(),
            Failing {
                since,
                last: Some(now),
                gone,
            },
        );
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

    /// Asks again, in the background, about every closed repository that
    /// is due: a "not installed" answer five minutes old (or named by an
    /// invalidation), and a repository GitHub did not show, five minutes
    /// after (or at once after an invalidation). So an App installed later
    /// opens its domains without a restart, even when no webhook says so.
    fn recheck_closed(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let now = Instant::now();
        let due: Vec<(String, Generation, bool)> = {
            let mut state = lock(&self.state);
            let not_installed = state
                .not_installed
                .iter()
                .filter(|(_, next)| now >= **next)
                .map(|(repo, _)| (repo.clone(), false));
            let gone = state
                .failing
                .iter()
                .filter(|(_, failing)| {
                    failing.gone && failing.last.is_none_or(|last| now >= last + FRESH_FOR)
                })
                .map(|(repo, _)| (repo.clone(), true));
            let due: Vec<(String, Generation, bool)> = not_installed
                .chain(gone)
                .filter(|(repo, _)| !state.rechecking.contains(repo))
                .map(|(repo, gone)| {
                    let generation = state.generation(&repo);
                    (repo, generation, gone)
                })
                .collect();
            state
                .rechecking
                .extend(due.iter().map(|(repo, _, _)| repo.clone()));
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
            for (repo, generation, gone) in due {
                let Some(source) = &source else {
                    lock(&me.state).rechecking.remove(&repo);
                    continue;
                };
                if gone {
                    // A plain lookup: a good one opens the domain, another
                    // "not there" keeps it closed for five more minutes.
                    me.snapshot(source, &repo).await;
                    lock(&me.state).rechecking.remove(&repo);
                    continue;
                }
                // Ask GitHub, not the App client's memory of the installation.
                source.evict_repo(&repo);
                let answer = tokio::time::timeout(REQUEST_TIMEOUT, source.installation_for(&repo))
                    .await
                    .ok();
                let mut state = lock(&me.state);
                state.rechecking.remove(&repo);
                // An invalidation since the question was asked already
                // decided what is known about this repository.
                if state.generation(&repo) != generation {
                    continue;
                }
                match answer {
                    Some(Ok(_)) => {
                        tracing::info!(
                            "the GitHub App is now installed on {repo}, so its domain's access follows GitHub"
                        );
                        state.not_installed.remove(&repo);
                    }
                    Some(Err(RemoteError::AppNotInstalled { .. })) => {
                        state
                            .not_installed
                            .insert(repo, Instant::now() + FRESH_FOR);
                    }
                    // Not known either way: it stays closed and is asked
                    // about again in five minutes.
                    Some(Err(_)) | None => {
                        state
                            .not_installed
                            .insert(repo, Instant::now() + FRESH_FOR);
                    }
                }
            }
        });
    }
}

/// One snapshot of `repo`: its collaborators, page by page, and whether it
/// is public. Each request is bounded by [`REQUEST_TIMEOUT`]; the whole
/// listing is not, so a large repository is listed however many pages it
/// takes.
async fn fetch(source: &dyn GithubSource, repo: &str) -> Result<Snapshot, FetchError> {
    async fn bounded<T>(
        request: impl std::future::Future<Output = Result<T, RemoteError>>,
    ) -> Result<T, FetchError> {
        match tokio::time::timeout(REQUEST_TIMEOUT, request).await {
            Ok(answer) => answer.map_err(FetchError::Remote),
            Err(_) => Err(FetchError::TimedOut),
        }
    }
    let mut people = HashMap::new();
    let mut page: Option<String> = None;
    let mut complete = false;
    for _ in 0..crystalline_remote::github::app::MAX_COLLABORATOR_PAGES {
        let (listed, next) = bounded(source.collaborators_page(repo, page.as_deref())).await?;
        people.extend(
            listed
                .into_iter()
                .map(|person| (person.id, (person.login, person.role))),
        );
        match next {
            Some(next) => page = Some(next),
            None => {
                complete = true;
                break;
            }
        }
    }
    if !complete {
        return Err(FetchError::Remote(
            crystalline_remote::github::app::too_many_collaborator_pages(repo),
        ));
    }
    let public = bounded(source.repository_public(repo)).await?;
    Ok(Snapshot { public, people })
}

#[async_trait]
impl KnowledgeGovernor for GithubGovernor {
    fn governed_domains(&self) -> HashSet<String> {
        // No origin, nothing to govern, and the App (whose first read may
        // ask the keychain) is not looked for at all.
        let origins = self.env.origins();
        if origins.is_empty() || matches!(self.env.app(), GovernorApp::Absent) {
            return HashSet::new();
        }
        self.recheck_closed();
        origins.into_iter().map(|(name, _)| name).collect()
    }

    fn closed_domains(&self) -> HashSet<String> {
        if matches!(self.env.app(), GovernorApp::Absent) {
            return HashSet::new();
        }
        let closed = self.closed_keys();
        if closed.is_empty() {
            return HashSet::new();
        }
        self.env
            .origins()
            .into_iter()
            .filter(|(_, repo)| closed.contains(&repo_key(repo)))
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
        match self.snapshot(&source, &repo).await {
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
        self.snapshot(&source, &repo)
            .await
            .is_some_and(|snapshot| snapshot.public)
    }

    async fn readers(&self, domain: &str) -> Option<HashSet<String>> {
        let repo = self.repo_of(domain)?;
        let access = self.env.access()?;
        let GovernorApp::Ready { source, host } = self.env.app() else {
            return None;
        };
        let snapshot = self.snapshot(&source, &repo).await?;
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

tokio::task_local! {
    /// Set while the MCP server serves one request (see [`as_mcp_call`]).
    static MCP_CALL: ();
}

/// Run `work` as one MCP request. Only inside it does an anonymous caller
/// count as the MCP open tier that acts as `github.agent_identity` on a
/// governed domain ([`Engine::scope_on`](super::Engine::scope_on)): the same `Scope::Anonymous` is
/// also the web's `auth.anonymous` visitor, who never gets the agent's
/// access. The narrowing (no open-tier carve-out on a governed domain)
/// holds for every anonymous caller, inside or outside.
pub async fn as_mcp_call<F: std::future::Future>(work: F) -> F::Output {
    MCP_CALL.scope((), work).await
}

/// Whether the current task serves an MCP request.
fn in_mcp_call() -> bool {
    MCP_CALL.try_with(|_| ()).is_ok()
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
        // Never a store or keychain read on a request: an unsettled slot is
        // loaded in the background, and until it lands an App that may be
        // saved governs with no access for anybody.
        let Some(slot) = engine.github_app_slot_settled() else {
            engine.preload_github_app();
            return if engine.github_app_maybe_saved() {
                GovernorApp::Unusable
            } else {
                GovernorApp::Absent
            };
        };
        match slot {
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
    /// `github.agent_identity` when that is set, see [`Engine::scope_on`](super::Engine::scope_on)).
    pub fn open_tier_carve_out(&self, scope: &Scope, domain: &str) -> bool {
        self.is_open_tier(scope) && !self.github_governs(domain)
    }

    /// The account the open tier acts as on the domains GitHub governs:
    /// `github.agent_identity`, when `scope` is the open tier, the current
    /// task serves an MCP request ([`as_mcp_call`]) and it is set.
    pub(crate) fn open_tier_agent(&self, scope: &Scope) -> Option<String> {
        // Only inside an MCP request: the web's anonymous visitor is
        // `Scope::Anonymous` too and must never act as the agent.
        if !self.is_open_tier(scope) || !in_mcp_call() {
            return None;
        }
        self.config
            .read()
            .unwrap()
            .github_agent_identity()
            .map(str::to_string)
    }

    /// The scope a question about `domain` is decided for: `scope` itself,
    /// except the open tier inside an MCP request ([`as_mcp_call`]) on a
    /// domain GitHub governs, which acts as `github.agent_identity` (as
    /// [`Scope::Agent`]: that account, clamped to editor) when that is set. Without it,
    /// and for every other anonymous caller, it stays anonymous there.
    pub fn scope_on(&self, scope: &Scope, domain: &str) -> Scope {
        match self.open_tier_agent(scope) {
            Some(account) if self.github_governs(domain) => Scope::Agent { account },
            _ => scope.clone(),
        }
    }

    /// Whether `caller` may open or answer a thread on `domain` when it
    /// holds `Read`: [`crate::scope::ReflectionCaller::may_comment`], except
    /// the open tier on a domain GitHub governs, which comments only when it
    /// acts there as `github.agent_identity` ([`Self::scope_on`]).
    pub fn open_tier_may_comment(
        &self,
        caller: &crate::scope::ReflectionCaller,
        domain: &str,
    ) -> bool {
        if caller.open_tier && self.github_governs(domain) {
            return matches!(self.scope_on(&caller.scope, domain), Scope::Agent { .. });
        }
        caller.may_comment()
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

    /// Since when a connected repository's lookups have been failing while
    /// its last answer may still serve (see [`GithubGovernor::stale_since`]).
    pub fn github_access_stale_since(&self) -> Option<DateTime<Utc>> {
        self.github_governor.get()?.stale_since()
    }

    /// The closed domains, each with its repository and the sentence that
    /// says what to do (see [`GithubGovernor::closed`]). Empty without an
    /// App.
    pub fn github_access_closed(&self) -> Vec<ClosedDomain> {
        self.github_governor
            .get()
            .map(|governor| governor.closed())
            .unwrap_or_default()
    }

    /// Whether `github.agent_identity` names an instance admin. The open
    /// tier never acts with admin rights (it is clamped to editor), but the
    /// setting is a mistake worth saying out loud. `false` without a
    /// resolver or when the store cannot be read.
    pub async fn github_agent_identity_is_admin(&self) -> bool {
        let agent = self
            .config
            .read()
            .unwrap()
            .github_agent_identity()
            .map(str::to_string);
        let (Some(agent), Some(access)) = (agent, self.domain_access.get()) else {
            return false;
        };
        access.account_is_admin(&agent).await.unwrap_or(false)
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
        evictions: AtomicUsize,
        /// While set, every call works out its answer and then waits here
        /// before it gives it: GitHub answered, the answer is on its way.
        gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
        /// How many calls are waiting at the gate.
        parked: AtomicUsize,
        /// While set, every listing page takes this long before it answers.
        delay: Mutex<Option<Duration>>,
        /// While set, how many people one listing page holds.
        page_size: Mutex<Option<usize>>,
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

        /// Holds every call at a gate from now on; returns the gate.
        fn park(&self) -> Arc<tokio::sync::Notify> {
            let gate = Arc::new(tokio::sync::Notify::new());
            *lock(&self.gate) = Some(gate.clone());
            gate
        }

        /// Waits (without letting the paused clock run) until `n` calls
        /// wait at the gate.
        async fn parked(&self, n: usize) {
            while self.parked.load(Ordering::SeqCst) < n {
                tokio::task::yield_now().await;
            }
        }

        async fn wait_at_gate(&self) {
            let gate = lock(&self.gate).clone();
            if let Some(gate) = gate {
                self.parked.fetch_add(1, Ordering::SeqCst);
                gate.notified().await;
            }
            let delay = *lock(&self.delay);
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
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
        /// Counts a listing on its first page.
        async fn collaborators_page(
            &self,
            repo: &str,
            page: Option<&str>,
        ) -> Result<(Vec<Collaborator>, Option<String>), RemoteError> {
            let index: usize = page.map_or(0, |page| page.parse().unwrap());
            if index == 0 {
                self.listings.fetch_add(1, Ordering::SeqCst);
            }
            let answer = match self.refusal(repo) {
                Some(e) => Err(e),
                None => {
                    let people = lock(&self.repos)
                        .get(&repo_key(repo))
                        .map(|r| r.people.clone())
                        .unwrap_or_default();
                    match *lock(&self.page_size) {
                        None => Ok((people, None)),
                        Some(size) => {
                            let chunk: Vec<Collaborator> = people
                                .iter()
                                .skip(index * size)
                                .take(size)
                                .cloned()
                                .collect();
                            let next = ((index + 1) * size < people.len())
                                .then(|| (index + 1).to_string());
                            Ok((chunk, next))
                        }
                    }
                }
            };
            self.wait_at_gate().await;
            answer
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
            let answer = match self.refusal(repo) {
                Some(e) => Err(e),
                None => Ok(7),
            };
            self.wait_at_gate().await;
            answer
        }

        fn evict_repo(&self, _repo: &str) {
            self.evictions.fetch_add(1, Ordering::SeqCst);
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
    async fn a_domain_on_a_repo_without_the_app_is_closed() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        assert!(rig.governor.governed_domains().contains(DOMAIN));
        assert!(rig.governor.closed_domains().is_empty(), "not known yet");
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        // Still governed, and closed now.
        assert!(rig.governor.governed_domains().contains(DOMAIN));
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(
            rig.governor.closed(),
            vec![ClosedDomain {
                domain: DOMAIN.to_string(),
                repo: REPO.to_string(),
                reason: format!(
                    "the GitHub App is not installed on {REPO}: install it there or disconnect the domain"
                ),
            }]
        );
        // Not installed is an answer, not an outage.
        assert_eq!(rig.governor.degraded_since(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_installed_later_opens_the_domain_without_a_restart() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.knowledge("alice").await;
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        rig.github.with(REPO, |r| r.not_installed = false);
        tokio::time::advance(FRESH_FOR).await;
        // The first question past five minutes asks again in the
        // background; until it is answered the domain stays closed.
        rig.governor.governed_domains();
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(rig.github.installation_lookups.load(Ordering::SeqCst), 1);
        assert!(rig.governor.closed_domains().is_empty());
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
    async fn invalidate_asks_again_about_an_installation_at_once() {
        for what in [
            Invalidate::Installation(7),
            Invalidate::Repo(REPO.to_string()),
            Invalidate::All,
        ] {
            let rig = rig().await;
            rig.github.with(REPO, |r| r.not_installed = true);
            rig.knowledge("alice").await;
            assert!(rig.governor.closed_domains().contains(DOMAIN));
            // Asked at once, and still not installed: stays closed.
            rig.governor.invalidate(what.clone());
            assert!(rig.governor.closed_domains().contains(DOMAIN), "{what:?}");
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert!(rig.governor.closed_domains().contains(DOMAIN), "{what:?}");
            // Installed now: the invalidation opens it without five minutes.
            rig.github.with(REPO, |r| r.not_installed = false);
            rig.governor.invalidate(what.clone());
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert!(rig.governor.closed_domains().is_empty(), "{what:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_renamed_github_account_keeps_its_access_and_its_new_login_is_stored() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-renamed", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        let link = rig.auth.github_link_of("alice").await.unwrap().unwrap();
        assert_eq!(link.login, "alice-renamed");
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalidation_during_a_lookup_is_not_undone_by_its_answer() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        tokio::time::advance(6 * MIN).await;
        // GitHub answers "write" and the answer is on its way ...
        let gate = rig.github.park();
        let governor = rig.governor.clone();
        let asking = tokio::spawn(async move { governor.knowledge("alice", DOMAIN).await });
        rig.github.parked(1).await;
        // ... when a webhook says alice was removed.
        rig.github.with(REPO, |r| r.people.clear());
        rig.governor.invalidate(Invalidate::Repo(REPO.to_string()));
        *lock(&rig.github.gate) = None;
        gate.notify_one();
        // The answer in flight serves the question it was asked for ...
        assert_eq!(asking.await.unwrap(), level(MemberLevel::Editor));
        // ... and is not kept: the next question asks GitHub again.
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.github.listings(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalidation_during_an_installation_recheck_is_not_undone() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.knowledge("alice").await;
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        tokio::time::advance(FRESH_FOR).await;
        // The re-check asks while the App is still not installed ...
        let gate = rig.github.park();
        rig.governor.governed_domains();
        rig.github.parked(1).await;
        // ... and the installation webhook lands before its answer.
        rig.github.with(REPO, |r| r.not_installed = false);
        rig.governor.invalidate(Invalidate::Installation(7));
        *lock(&rig.github.gate) = None;
        gate.notify_one();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        // The stale "not installed" was not stored for five more minutes:
        // the next question asks again at once and the domain opens.
        rig.governor.governed_domains();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(rig.governor.closed_domains().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_caller_answers_from_a_recent_snapshot_while_a_lookup_is_in_flight() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        tokio::time::advance(6 * MIN).await;
        let gate = rig.github.park();
        let governor = rig.governor.clone();
        let asking = tokio::spawn(async move { governor.knowledge("alice", DOMAIN).await });
        rig.github.parked(1).await;
        let before = Instant::now();
        let answer = tokio::time::timeout(Duration::from_secs(1), rig.knowledge("alice"))
            .await
            .expect("answered without waiting for the lookup");
        assert_eq!(answer, level(MemberLevel::Editor));
        assert_eq!(Instant::now(), before);
        *lock(&rig.github.gate) = None;
        gate.notify_one();
        asking.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_takes_too_long_is_a_failure() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        tokio::time::advance(6 * MIN).await;
        // GitHub never answers: the last answer stands, and it is degraded.
        let _gate = rig.github.park();
        let before = Instant::now();
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(Instant::now() - before, FETCH_TIMEOUT);
        // The page timed out at the same moment; the lookup records it.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(rig.governor.degraded_since().is_some());
        // With nothing known it is no access.
        let rig = self::rig().await;
        let _gate = rig.github.park();
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
    }

    #[tokio::test(start_paused = true)]
    async fn the_pause_after_a_failure_counts_from_when_it_failed() {
        let rig = rig().await;
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        *lock(&rig.github.delay) = Some(Duration::from_secs(8));
        rig.knowledge("alice").await;
        // Failed at 8 s. At 33 s the pause (30 s from the failure) holds.
        tokio::time::advance(Duration::from_secs(25)).await;
        rig.knowledge("alice").await;
        assert_eq!(rig.github.listings(), 1);
        tokio::time::advance(Duration::from_secs(6)).await;
        rig.knowledge("alice").await;
        assert_eq!(rig.github.listings(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn an_invalidation_ends_the_pause_after_a_failure() {
        let rig = rig().await;
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        rig.knowledge("alice").await;
        rig.governor.invalidate(Invalidate::Repo(REPO.to_string()));
        rig.knowledge("alice").await;
        assert_eq!(rig.github.listings(), 2);
        // Still degraded: the second lookup failed too.
        assert!(rig.governor.degraded_since().is_some());
        // `All` forgets the outage until a lookup fails again.
        rig.governor.invalidate(Invalidate::All);
        assert_eq!(rig.governor.degraded_since(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_gone_repository_is_closed_and_its_installation_looked_up_again() {
        let rig = rig().await;
        rig.github.with(REPO, |r| {
            r.down = Some(|| RemoteError::RepoNotFound {
                repo: REPO.to_string(),
            })
        });
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.github.evictions.load(Ordering::SeqCst), 1);
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(rig.governor.closed().len(), 1);
        assert_eq!(rig.governor.stale_since(), None);
        assert!(rig.governor.degraded_since().is_some());
        // An outage after it keeps it closed: only a good lookup opens it.
        rig.github
            .with(REPO, |r| r.down = Some(|| RemoteError::Offline));
        tokio::time::advance(MIN).await;
        rig.knowledge("alice").await;
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        // GitHub shows it again: the re-check five minutes on opens it.
        rig.github.with(REPO, |r| r.down = None);
        tokio::time::advance(FRESH_FOR).await;
        rig.governor.governed_domains();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(rig.governor.closed_domains().is_empty());
    }

    /// A large repository: three pages of six seconds each take longer than
    /// a question waits, but each page answers in time. The question is
    /// answered from what is known, and the lookup goes on and stores its
    /// snapshot for the next one.
    #[tokio::test(start_paused = true)]
    async fn a_slow_listing_is_stored_after_the_question_that_started_it() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.github.grant(REPO, 2, "bob", RepoRole::Read);
        rig.github.grant(REPO, 3, "carol", RepoRole::Read);
        *lock(&rig.github.page_size) = Some(1);
        *lock(&rig.github.delay) = Some(Duration::from_secs(6));
        let before = Instant::now();
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(Instant::now() - before, FETCH_TIMEOUT);
        // The lookup finishes in the background.
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.github.listings(), 1);
        assert_eq!(rig.governor.degraded_since(), None);
    }
}
