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
use crystalline_remote::state::RepositoryIds;
use crystalline_remote::{
    Collaborator, GithubApp, RemoteError, RepoRole, RepoVisibility, RepositoryFacts,
};
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

/// How long a domain [`GithubGovernor::close`] closed stays closed before
/// the background recheck asks GitHub whether the App still reaches it, so a
/// GitHub that has not caught up with its own event does not open it again
/// at once.
pub const CLOSE_GRACE: Duration = Duration::from_secs(60);

/// How long the two questions that confirm a rename with GitHub may take
/// together, well inside the ten seconds GitHub waits for a webhook's
/// answer. Past it the rename is not followed (both names are forgotten,
/// and GitHub's redirect keeps the domain working).
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);

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

/// One person GitHub lists on a governed domain's repository, as the Members
/// card draws them (see [`Engine::github_people`](super::Engine::github_people)).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GithubPerson {
    /// The login GitHub lists.
    pub login: String,
    /// Their role on the repository: `admin`, `maintain`, `write`,
    /// `triage` or `read`.
    pub github_role: String,
    /// The level that role gives here ([`level_for`]).
    pub level: MemberLevel,
    /// The enabled account linked to that GitHub id on this App's host, or
    /// `None` when nobody here is linked to it.
    pub account: Option<String>,
}

/// Who reads a repository on GitHub, for the Reflection source check
/// (Task 3.7): whether it is public or internal, and the GitHub ids with
/// read or more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoReaders {
    /// Whether the repository is public, so everybody reads it.
    pub public: bool,
    /// Whether it is internal (GitHub Enterprise): every member of the
    /// enterprise reads it, and the listing does not name them.
    pub internal: bool,
    /// The GitHub ids of every collaborator with read or more: who reads it
    /// for certain, what a source repository counts.
    pub github_ids: HashSet<u64>,
    /// The GitHub ids of every person the listing names, whatever their
    /// role resolves to here (a custom role read as no access included):
    /// who may read it, what a page repository counts.
    pub listed_ids: HashSet<u64>,
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
    /// Who may read `repo`, its ids and its current name, see
    /// [`GithubApp::repository_facts`].
    async fn repository_facts(&self, repo: &str) -> Result<RepositoryFacts, RemoteError>;
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

    async fn repository_facts(&self, repo: &str) -> Result<RepositoryFacts, RemoteError> {
        GithubApp::repository_facts(self, repo).await
    }

    async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError> {
        GithubApp::installation_for(self, repo).await
    }

    fn evict_repo(&self, repo: &str) {
        GithubApp::evict_repo(self, repo);
    }
}

/// What is recorded about a domain's repository, for the governor's check
/// of who the repository is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recorded {
    /// The ids recorded when it was connected (or on its first lookup).
    Ids(RepositoryIds),
    /// Nothing yet: a domain connected before the ids were recorded.
    Nothing,
    /// The origin state could not be read: the check fails closed.
    Unreadable,
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
    /// A recheck found that GitHub shows a closed repository to the App
    /// again, so its domain is open: who reads it changed. The engine runs
    /// the Reflection rescan a visibility change runs.
    fn reopened(&self) {}
    /// Every domain connected to `repo`, with what is recorded for it. A
    /// domain with no origin state at all is left out.
    fn recorded_ids(&self, _repo: &str) -> Vec<(String, Recorded)> {
        Vec::new()
    }
    /// Record `ids` for `domain`, which had none: trust on first use, for a
    /// domain connected before the ids were recorded at connect.
    fn record_ids(&self, _domain: &str, _ids: RepositoryIds) {}
    /// GitHub answers `old` under the name `live` (renamed within its owner,
    /// the ids unchanged): the engine may connect the domains to `live`.
    fn renamed(&self, _old: &str, _live: &str) {}
}

/// One listing of a repository: whether it is public and, by GitHub id,
/// each collaborator's login and role.
#[derive(Debug)]
struct Snapshot {
    public: bool,
    /// Internal (GitHub Enterprise): not public, and read by members of the
    /// enterprise the listing does not name.
    internal: bool,
    people: HashMap<u64, (String, RepoRole)>,
    /// The repository's id and its owner's id, when GitHub sent them.
    ids: Option<RepositoryIds>,
    /// Its current name, when GitHub sent it.
    live_name: Option<String>,
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
    /// Why a repository in `not_installed` is closed, when a webhook said
    /// so ([`GithubGovernor::close`]); without an entry the reason is that
    /// the App is not installed. Goes when the domain opens.
    reasons: HashMap<String, String>,
    /// Repositories that moved to another owner on GitHub
    /// ([`GithubGovernor::close_moved`]), with the reason. Closed for good:
    /// no recheck and no invalidation opens them, only a reconnect (which
    /// names another repository).
    moved: HashMap<String, String>,
    /// Repositories whose last lookup found ids other than the ones
    /// recorded for their domain (transferred, or a reused name), with the
    /// reason and when to ask again. Closed; decided again on every lookup,
    /// so a restart closes them again on the first one.
    mismatched: HashMap<String, (String, Instant)>,
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
        let (reasons, moved) = {
            let state = lock(&self.state);
            let mut moved = state.moved.clone();
            for (key, (reason, _)) in &state.mismatched {
                moved.entry(key.clone()).or_insert_with(|| reason.clone());
            }
            (state.reasons.clone(), moved)
        };
        let mut out: Vec<ClosedDomain> = self
            .env
            .origins()
            .into_iter()
            .filter(|(_, repo)| closed.contains(&repo_key(repo)))
            .map(|(domain, repo)| {
                let key = repo_key(&repo);
                let reason = moved
                    .get(&key)
                    .or_else(|| reasons.get(&key))
                    .cloned()
                    .unwrap_or_else(|| {
                        format!(
                            "the GitHub App is not installed on {repo}: install it there or disconnect the domain"
                        )
                    });
                ClosedDomain {
                    reason,
                    domain,
                    repo,
                }
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
            .chain(state.moved.keys())
            .chain(state.mismatched.keys())
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

    /// Starts one background lookup for every governed repository that has
    /// no fresh snapshot and is not known to lack the App, so a repository
    /// without the App is closed before anybody asks about it, and the
    /// first questions find a snapshot. Each lookup takes the per-repo gate
    /// and the generation check like any other; nothing waits on them.
    /// Called when the App has been loaded (at start and after it changed).
    /// A no-op without a usable App or outside a tokio runtime.
    pub fn warm_up(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let GovernorApp::Ready { source, .. } = self.env.app() else {
            return;
        };
        let now = Instant::now();
        let repos: HashSet<String> = {
            let state = lock(&self.state);
            self.env
                .origins()
                .into_iter()
                .map(|(_, repo)| repo)
                .filter(|repo| {
                    let key = repo_key(repo);
                    !state.not_installed.contains_key(&key)
                        && !state
                            .entries
                            .get(&key)
                            .is_some_and(|entry| now < entry.fetched_at + FRESH_FOR)
                })
                .collect()
        };
        for repo in repos {
            let Some(me) = self.me.upgrade() else {
                return;
            };
            let source = source.clone();
            runtime.spawn(async move {
                me.snapshot(&source, &repo).await;
            });
        }
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
        // Asked before the lock: `All` with no usable App (it changed, or
        // the new one is not loaded yet) drops the old App's closed set, so
        // the base rules apply until the new one answers (R21).
        let ready =
            matches!(what, Invalidate::All) && matches!(self.env.app(), GovernorApp::Ready { .. });
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
                    if let Some((_, next)) = state.mismatched.get_mut(&key) {
                        *next = now;
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
                        .mismatched
                        .values_mut()
                        .for_each(|(_, next)| *next = now);
                    state
                        .not_installed
                        .values_mut()
                        .for_each(|next| *next = now);
                    state.failing.values_mut().for_each(|f| f.last = None);
                }
                Invalidate::All if !ready => {
                    state.global += 1;
                    state.entries.clear();
                    state.not_installed.clear();
                    state.reasons.clear();
                    state.mismatched.clear();
                    state.failing.clear();
                }
                Invalidate::All => {
                    state.global += 1;
                    state.entries.clear();
                    state
                        .mismatched
                        .values_mut()
                        .for_each(|(_, next)| *next = now);
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

    /// Close `repo`'s domain at once, because GitHub said the App lost it
    /// (the repository was deleted, taken out of the installation, or the
    /// installation went away), with `reason` as the doctor and the admin
    /// banner name it. Its snapshot is forgotten, a lookup in flight is not
    /// stored, and only the machine owner and instance admins get in. After
    /// [`CLOSE_GRACE`] the usual recheck asks GitHub in the background
    /// whether the App is installed on it, and opens the domain again only
    /// when GitHub says so, so a webhook payload can narrow access but never
    /// widen it. An invalidation that names it asks at once.
    pub fn close(&self, repo: &str, reason: String) {
        {
            let mut state = lock(&self.state);
            let key = repo_key(repo);
            *state.generations.entry(key.clone()).or_default() += 1;
            state.entries.remove(&key);
            state.failing.remove(&key);
            state
                .not_installed
                .insert(key.clone(), Instant::now() + CLOSE_GRACE);
            state.reasons.insert(key, reason);
        }
        self.recheck_closed();
    }

    /// Close `repo`'s domain for good, because the repository moved to
    /// another owner on GitHub (Ruling R32: a transfer is never followed on
    /// a payload's word). No recheck and no invalidation opens it again in
    /// this process; an admin reconnects the domain to the repository where
    /// it is now. This is the webhook's fast path, held in memory: what
    /// keeps the domain closed across a restart is the comparison of the
    /// recorded repository and owner ids on every lookup (Ruling R33).
    pub fn close_moved(&self, repo: &str, reason: String) {
        let mut state = lock(&self.state);
        let key = repo_key(repo);
        *state.generations.entry(key.clone()).or_default() += 1;
        state.entries.remove(&key);
        state.failing.remove(&key);
        state.not_installed.remove(&key);
        state.reasons.remove(&key);
        state.moved.insert(key, reason);
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
            internal: snapshot.internal,
            github_ids: snapshot
                .people
                .iter()
                .filter(|(_, (_, role))| level_for(*role) != MemberLevel::None)
                .map(|(id, _)| *id)
                .collect(),
            listed_ids: snapshot.people.keys().copied().collect(),
        })
    }

    /// The people GitHub lists on `domain`'s repository, each joined to the
    /// enabled account linked to its GitHub id on this App's host, sorted by
    /// login, from the same snapshot as every other answer (so the five
    /// minute cache holds: no extra request to GitHub). `None` when that is
    /// not known right now, as for [`GithubGovernor::repository_readers`].
    pub async fn repository_people(&self, domain: &str) -> Option<Vec<GithubPerson>> {
        let repo = self.repo_of(domain)?;
        let GovernorApp::Ready { source, host } = self.env.app() else {
            return None;
        };
        let snapshot = self.snapshot(&source, &repo).await?;
        let mut accounts: HashMap<u64, String> = HashMap::new();
        if let Some(access) = self.env.access() {
            let links = access
                .github_links()
                .await
                .inspect_err(|e| tracing::warn!("the GitHub links could not be read: {e:#}"))
                .unwrap_or_default();
            let enabled = access
                .enabled_accounts()
                .await
                .inspect_err(|e| tracing::warn!("the accounts could not be read: {e:#}"))
                .unwrap_or_default();
            for link in links {
                if on_host(&link, &host) && enabled.contains(&link.account) {
                    accounts.insert(link.github_id, link.account);
                }
            }
        }
        let mut people: Vec<GithubPerson> = snapshot
            .people
            .iter()
            .filter(|(_, (_, role))| level_for(*role) != MemberLevel::None)
            .map(|(id, (login, role))| GithubPerson {
                login: login.clone(),
                github_role: role.as_str().to_string(),
                level: level_for(*role),
                account: accounts.get(id).cloned(),
            })
            .collect();
        people.sort_by_key(|person| person.login.to_lowercase());
        Some(people)
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
            if state.not_installed.contains_key(&key) || state.moved.contains_key(&key) {
                return None;
            }
            if state
                .mismatched
                .get(&key)
                .is_some_and(|(_, next)| started < *next)
            {
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
                if let Some(reason) = self.identity_differs(repo, &snapshot) {
                    let mut state = lock(&self.state);
                    state.failing.remove(&key);
                    state.entries.remove(&key);
                    if state.generation(&key) == generation {
                        if !state.mismatched.contains_key(&key) {
                            tracing::warn!(
                                "{reason}; the domain of {repo} is closed to all but instance admins"
                            );
                        }
                        state
                            .mismatched
                            .insert(key, (reason, Instant::now() + FRESH_FOR));
                    }
                    return None;
                }
                let snapshot = Arc::new(snapshot);
                {
                    let mut state = lock(&self.state);
                    if state.mismatched.remove(&key).is_some() {
                        tracing::info!(
                            "GitHub shows the repository recorded for {repo} again, so its domain is open"
                        );
                    }
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

    /// Whether the repository GitHub answered for `repo` is not the one its
    /// domains were connected to: the reason when it is not, `None` when it
    /// is (or nothing is recorded or answered to compare). An origin state
    /// that cannot be read fails closed. A domain with no recorded ids
    /// records the answered ones (trust on first use). A
    /// rename within the owner (the same ids under another name) is not a
    /// difference: the engine is told, and may follow it, but only when
    /// every domain on `repo` had its ids recorded and they match.
    fn identity_differs(&self, repo: &str, snapshot: &Snapshot) -> Option<String> {
        let live = snapshot.ids?;
        let key = repo_key(repo);
        let connected = self
            .env
            .origins()
            .iter()
            .filter(|(_, r)| repo_key(r) == key)
            .count();
        let mut matched = 0;
        for (domain, recorded) in self.env.recorded_ids(repo) {
            match recorded {
                Recorded::Unreadable => {
                    return Some(format!("the origin state of {domain} cannot be read"));
                }
                Recorded::Nothing => {
                    tracing::info!(
                        "the repository ids of {repo} are recorded for the domain {domain} on its first lookup"
                    );
                    self.env.record_ids(&domain, live);
                }
                Recorded::Ids(recorded) if recorded.repository != live.repository => {
                    return Some(format!(
                        "the name {repo} now belongs to another repository on GitHub; disconnect the domain and connect it again"
                    ));
                }
                Recorded::Ids(recorded) if recorded.owner != live.owner => {
                    let to = snapshot.live_name.as_deref().unwrap_or(repo);
                    return Some(format!(
                        "the repository moved to {to} on GitHub; disconnect the domain and connect it again"
                    ));
                }
                Recorded::Ids(_) => matched += 1,
            }
        }
        // Followed only when every domain on the repository had its ids
        // recorded and they match: a domain left out (no state) or recorded
        // just now proves nothing.
        if connected > 0
            && matched == connected
            && let Some(live_name) = &snapshot.live_name
            && repo_key(live_name) != key
        {
            self.env.renamed(repo, live_name);
        }
        None
    }

    /// An admin connected a domain to `repo` again (and its ids were
    /// recorded anew): whatever closed it is forgotten and the next
    /// question asks GitHub.
    pub fn reconnected(&self, repo: &str) {
        {
            let mut state = lock(&self.state);
            let key = repo_key(repo);
            state.moved.remove(&key);
            state.mismatched.remove(&key);
            state.reasons.remove(&key);
            state.not_installed.remove(&key);
        }
        self.invalidate(Invalidate::Repo(repo.to_string()));
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
            let mut reopened = false;
            for (repo, generation, gone) in due {
                let Some(source) = &source else {
                    // Nobody to ask: put it off five minutes rather than
                    // start it again on every question.
                    let mut state = lock(&me.state);
                    state.rechecking.remove(&repo);
                    let now = Instant::now();
                    if let Some(next) = state.not_installed.get_mut(&repo) {
                        *next = now + FRESH_FOR;
                    }
                    if let Some(failing) = state.failing.get_mut(&repo) {
                        failing.last = Some(now);
                    }
                    continue;
                };
                if gone {
                    // A plain lookup: a good one opens the domain, another
                    // "not there" keeps it closed for five more minutes.
                    me.snapshot(source, &repo).await;
                    lock(&me.state).rechecking.remove(&repo);
                    if !me.closed_keys().contains(&repo) {
                        reopened = true;
                    }
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
                        state.reasons.remove(&repo);
                        reopened = true;
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
            if reopened {
                me.env.reopened();
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
    let facts = bounded(source.repository_facts(repo)).await?;
    Ok(Snapshot {
        public: facts.visibility == RepoVisibility::Public,
        internal: facts.visibility == RepoVisibility::Internal,
        people,
        ids: facts.ids,
        live_name: facts.full_name,
    })
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
        match self.engine.upgrade() {
            Some(engine) => engine.github_origins(),
            None => Vec::new(),
        }
    }

    fn access(&self) -> Option<Arc<DomainAccess>> {
        self.engine.upgrade()?.domain_access.get().cloned()
    }

    fn recorded_ids(&self, repo: &str) -> Vec<(String, Recorded)> {
        let Some(engine) = self.engine.upgrade() else {
            return Vec::new();
        };
        let key = repo_key(repo);
        engine
            .github_origins()
            .into_iter()
            .filter(|(_, connected)| repo_key(connected) == key)
            .filter_map(|(domain, _)| {
                let Ok(dir) = engine.origin_state_dir(&domain) else {
                    return Some((domain, Recorded::Unreadable));
                };
                match crystalline_remote::state::OriginState::load(&dir) {
                    Ok(Some(state)) => Some((
                        domain,
                        state
                            .repository_ids
                            .map_or(Recorded::Nothing, Recorded::Ids),
                    )),
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(
                            "the origin state of the domain {domain} could not be read to compare its repository ids, so it is closed: {e}"
                        );
                        Some((domain, Recorded::Unreadable))
                    }
                }
            })
            .collect()
    }

    fn record_ids(&self, domain: &str, ids: RepositoryIds) {
        let (Some(engine), Ok(runtime)) =
            (self.engine.upgrade(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        let Ok(dir) = engine.origin_state_dir(domain) else {
            return;
        };
        let lock = engine.origin_lock(domain);
        let domain = domain.to_string();
        runtime.spawn(async move {
            let _held = lock.lock().await;
            if let Err(e) = write_repository_ids(&dir, ids, false) {
                tracing::warn!(
                    "the repository ids of the domain {domain} could not be recorded: {e}"
                );
            }
        });
    }

    fn renamed(&self, old: &str, live: &str) {
        let (Some(engine), Ok(runtime)) =
            (self.engine.upgrade(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        if engine.read_only {
            return;
        }
        let owner = |name: &str| name.split_once('/').map(|(o, _)| o.to_ascii_lowercase());
        if owner(old).is_none() || owner(old) != owner(live) {
            tracing::warn!(
                "GitHub answers {old} as {live}, under another owner; that is never followed on its own"
            );
            return;
        }
        let old_key = repo_key(old);
        let live = live.to_string();
        runtime.spawn(async move {
            if let Err(e) = engine.move_origin_repo(&old_key, &live, &HashSet::new()) {
                tracing::warn!(
                    "the domains of {old_key} could not be connected to its new name {live}: {e}"
                );
            }
        });
    }

    /// A domain opened again: look for Reflection pages its readers now
    /// make too wide and schedule their backup, as after a visibility
    /// change (`Engine::schedule_too_wide_backups`).
    fn reopened(&self) {
        let (Some(engine), Ok(runtime)) =
            (self.engine.upgrade(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        runtime.spawn(async move {
            engine.schedule_too_wide_backups().await;
        });
    }
}

impl super::Engine {
    /// The GitHub governor of this engine, built on first use. Every caller
    /// gets the same one, so every [`DomainAccess`] answers from one cache.
    ///
    /// When the App slot is already settled (a fast load, or an origin
    /// operation loaded it), building the governor warms it up at once
    /// ([`GithubGovernor::warm_up`]), since no later load will.
    pub fn github_governor(self: &Arc<Self>) -> Arc<GithubGovernor> {
        let mut built = false;
        let governor = self
            .github_governor
            .get_or_init(|| {
                built = true;
                GithubGovernor::new(Arc::new(EngineEnv {
                    engine: Arc::downgrade(self),
                }))
            })
            .clone();
        if built && self.github_app_slot_settled().is_some() {
            governor.warm_up();
        }
        governor
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

    /// Whether a GitHub App decides access to `domain` on this instance,
    /// asked of the App store and the domain's origin rather than of the
    /// resolver: an App is configured (usable or not) and `domain` has a
    /// GitHub origin, the rule the governor follows. Unlike
    /// [`Self::github_governs`] it holds on a daemon that serves no HTTP
    /// too, where the stored records still decide the day it does. The
    /// CLI's `domain visibility` and `domain members` ask a running daemon
    /// this, so a daemon that has the App from its own environment decides
    /// for a command run in a shell that has none.
    pub async fn github_app_decides_access(self: &Arc<Self>, domain: &str) -> bool {
        if !self.github_origins().iter().any(|(name, _)| name == domain) {
            return false;
        }
        // The first read of the App may ask the keychain.
        let engine = self.clone();
        tokio::task::spawn_blocking(move || engine.github_app_configured())
            .await
            .unwrap_or(true)
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

    /// Why `domain` is closed (see [`GithubGovernor::closed`]), or `None`
    /// when it is open or not governed.
    pub fn github_closed_reason(&self, domain: &str) -> Option<String> {
        self.github_access_closed()
            .into_iter()
            .find(|closed| closed.domain == domain)
            .map(|closed| closed.reason)
    }

    /// The people GitHub lists on `domain`'s repository, joined to the
    /// accounts linked to them (see [`GithubGovernor::repository_people`]).
    /// `None` for a domain GitHub does not govern, and while GitHub's answer
    /// is not known.
    pub async fn github_people(self: &Arc<Self>, domain: &str) -> Option<Vec<GithubPerson>> {
        if !self.github_governs(domain) {
            return None;
        }
        self.github_governor().repository_people(domain).await
    }

    /// The `CRYSTALLINE_DOMAIN_<NAME>_READERS` variable that decides who may
    /// read `domain`, when `domain` is defined in the environment with an
    /// origin. Such a domain's readers are applied again on every start
    /// (see [`Engine::apply_env_readers`](super::Engine::apply_env_readers)),
    /// so a change made anywhere else would not last.
    pub fn env_readers_var(&self, domain: &str) -> Option<String> {
        self.overlay
            .env_domains()
            .find(|(name, env)| name.as_str() == domain && env.entry.origin.is_some())
            .map(|(_, env)| env.readers_var())
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

    /// Every registered domain with an origin, with its repository
    /// (`owner/name`): the configured ones, then the discovered ones the
    /// configuration does not name.
    pub(crate) fn github_origins(&self) -> Vec<(String, String)> {
        let config = self.config.read().unwrap();
        let mut origins: Vec<(String, String)> = config
            .domains
            .iter()
            .filter_map(|(name, entry)| Some((name.clone(), entry.origin.as_ref()?.repo.clone())))
            .collect();
        for (name, entry) in self.discovered_domains.read().unwrap().iter() {
            if config.domains.contains_key(name) {
                continue;
            }
            if let Some(origin) = &entry.origin {
                origins.push((name.clone(), origin.repo.clone()));
            }
        }
        origins
    }

    /// Records the ids of `repo` (and its owner) for `domain`, just
    /// connected, in its origin state under `state_dir`, replacing any
    /// recorded before, and tells the governor the domain was connected
    /// again, so whatever closed it is forgotten. Called by a connect that
    /// holds the domain's origin lock. Without a loaded App, or when GitHub
    /// does not answer, nothing is recorded and the first good lookup
    /// records them instead (trust on first use).
    pub(super) async fn record_repository_ids_at_connect(
        &self,
        domain: &str,
        repo: &str,
        state_dir: &std::path::Path,
    ) {
        if let Some(super::github_app::AppSlot::Ready(router)) = self.github_app_slot_settled() {
            match tokio::time::timeout(REQUEST_TIMEOUT, router.app().repository_facts(repo)).await {
                Ok(Ok(RepositoryFacts { ids: Some(ids), .. })) => {
                    if let Err(e) = write_repository_ids(state_dir, ids, true) {
                        tracing::warn!(
                            "the repository ids of the domain {domain} could not be recorded at connect: {e}"
                        );
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::info!(
                    "the repository ids of {repo} could not be read at connect ({e}); the first lookup records them"
                ),
                Err(_) => tracing::info!(
                    "GitHub was slow to answer the repository ids of {repo} at connect; the first lookup records them"
                ),
            }
        }
        if let Some(governor) = self.github_governor.get() {
            governor.reconnected(repo);
        }
    }

    /// Whether a registered domain is connected to `repo` (GitHub names are
    /// case insensitive).
    fn github_repo_connected(&self, repo: &str) -> bool {
        let key = repo_key(repo);
        self.github_origins()
            .iter()
            .any(|(_, connected)| repo_key(connected) == key)
    }

    /// Handle one verified GitHub App webhook delivery: event `event` (the
    /// `X-GitHub-Event` header) with its JSON `payload`.
    ///
    /// The signature proves GitHub wrote the body, not that it is new or
    /// still true: it covers the body only, so a captured delivery can be
    /// sent again with a fresh delivery id, which the replay memory does not
    /// catch, long after what it says stopped being so. So every event here
    /// is idempotent and only ever narrows: it makes the governor forget
    /// what it knew (the next question asks GitHub), or closes a domain at
    /// once. Nothing in a payload is believed about who may do what: no
    /// role, permission or visibility is read from it, only which
    /// repository, person or installation to forget or close, and a domain
    /// is moved to another repository only when GitHub itself confirms it.
    /// Events about repositories no domain is connected to do nothing. What
    /// is handled:
    ///
    /// * `member`, `team` (`added_to_repository`, `removed_from_repository`,
    ///   `edited`, `deleted`) and `repository` `privatized`/`publicized`:
    ///   the repository is forgotten ([`Invalidate::Repo`]); a team event
    ///   that names no repository forgets every connected one.
    /// * `membership` (`added`, `removed`) and `organization`
    ///   (`member_added`, `member_removed`): the person is forgotten
    ///   ([`Invalidate::User`]).
    /// * `repository` `renamed`: both names are forgotten, and the domains
    ///   connected to the old name are connected to the new one only when
    ///   GitHub, asked with the installation token, answers the same
    ///   repository id for `repository.id` and for the old name (through
    ///   its redirect) and the name it gives is the payload's.
    /// * `repository` `transferred` to another owner is never followed
    ///   (Ruling R32): the domain is closed for good
    ///   ([`GithubGovernor::close_moved`]) until an admin reconnects it. A
    ///   transfer that keeps the owner's name is a rename.
    /// * `repository` `deleted`, `installation_repositories` with
    ///   repositories removed, and `installation` `deleted`/`suspend` (every
    ///   repository the payload lists or the App client knows under that
    ///   installation): the domains are closed at once
    ///   ([`GithubGovernor::close`]), each with its reason.
    /// * `installation` (any other action) and `installation_repositories`
    ///   with repositories added: the App client forgets the installation
    ///   and the governor rechecks every closed domain
    ///   ([`Invalidate::Installation`]). A domain opens again only when
    ///   GitHub shows the App its repository, and its opening runs the
    ///   Reflection rescan.
    /// * `github_app_authorization` `revoked`: the person's user token is
    ///   deleted and their link kept (`sender.id` names them).
    ///
    /// The answer says whether who reads a connected domain may have
    /// changed, so the surface runs the Reflection rescan a visibility
    /// change runs. Fails only when a change could not be saved (the
    /// configuration after a rename, the token store after a revocation).
    pub async fn apply_github_event(
        self: &Arc<Self>,
        event: &str,
        payload: &serde_json::Value,
    ) -> super::Result<GithubEventOutcome> {
        let action = payload
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let governor = self.github_governor();
        let readers_changed = match (event, action) {
            ("member", _) | ("repository", "privatized" | "publicized") => {
                match repository_of(payload) {
                    Some(repo) => self.forget_github_repo(&governor, &repo),
                    None => self.forget_every_github_repo(&governor),
                }
            }
            ("team", "added_to_repository" | "removed_from_repository" | "edited" | "deleted") => {
                match repository_of(payload) {
                    Some(repo) => self.forget_github_repo(&governor, &repo),
                    None => self.forget_every_github_repo(&governor),
                }
            }
            ("membership", "added" | "removed") => {
                self.forget_github_user(&governor, payload.pointer("/member/id"))
            }
            ("organization", "member_added" | "member_removed") => {
                self.forget_github_user(&governor, payload.pointer("/membership/user/id"))
            }
            ("repository", "renamed") => match (
                repository_of(payload),
                renamed_from("renamed", payload),
            ) {
                (Some(new), Some(old)) => self.follow_github_rename(&governor, &old, &new, payload).await?,
                (Some(new), None) => self.forget_github_repo(&governor, &new),
                _ => false,
            },
            ("repository", "transferred") => {
                self.github_repo_transferred(&governor, payload).await?
            }
            ("repository", "deleted") => match repository_of(payload) {
                Some(repo) => self.close_github_repos(&governor, [repo], |repo| {
                    format!(
                        "{repo} was deleted on GitHub: disconnect the domain or connect it to another repository"
                    )
                }),
                None => false,
            },
            ("installation", _) => {
                let installation = installation_of(payload);
                let closed = if matches!(action, "deleted" | "suspend") {
                    let mut repos = repositories_in(payload, "repositories");
                    repos.extend(self.github_installation_repositories(installation));
                    self.close_github_repos(&governor, repos, |repo| {
                        format!(
                            "the GitHub App's installation that covers {repo} was removed or suspended: install the App again or disconnect the domain"
                        )
                    })
                } else {
                    false
                };
                self.evict_github_installation(installation);
                governor.invalidate(match installation {
                    Some(id) => Invalidate::Installation(id),
                    None => Invalidate::All,
                });
                closed
            }
            ("installation_repositories", _) => {
                let installation = installation_of(payload);
                let removed = self.close_github_repos(
                    &governor,
                    repositories_in(payload, "repositories_removed"),
                    |repo| {
                        format!(
                            "{repo} was removed from the GitHub App's installation: add it back there or disconnect the domain"
                        )
                    },
                );
                let added = repositories_in(payload, "repositories_added")
                    .iter()
                    .any(|repo| self.github_repo_connected(repo));
                if added {
                    // Opens only through the recheck, which asks GitHub and
                    // runs the Reflection rescan when it opens a domain.
                    self.evict_github_installation(installation);
                    governor.invalidate(match installation {
                        Some(id) => Invalidate::Installation(id),
                        None => Invalidate::All,
                    });
                }
                removed
            }
            ("github_app_authorization", "revoked") => {
                self.github_app_revoked_by(payload.pointer("/sender/id"))
                    .await?;
                false
            }
            _ => false,
        };
        Ok(GithubEventOutcome { readers_changed })
    }

    /// Forgets `repo` when a domain is connected to it. Answers whether one
    /// is.
    fn forget_github_repo(&self, governor: &GithubGovernor, repo: &str) -> bool {
        if !self.github_repo_connected(repo) {
            return false;
        }
        governor.invalidate(Invalidate::Repo(repo.to_string()));
        true
    }

    /// Forgets every connected repository, for an event that may change
    /// access but does not say where. Answers whether any is connected.
    fn forget_every_github_repo(&self, governor: &GithubGovernor) -> bool {
        let origins = self.github_origins();
        for (_, repo) in &origins {
            governor.invalidate(Invalidate::Repo(repo.clone()));
        }
        !origins.is_empty()
    }

    /// Forgets the GitHub user `id` names, or every connected repository
    /// when the payload carries no usable id. Answers whether any domain is
    /// connected.
    fn forget_github_user(
        &self,
        governor: &GithubGovernor,
        id: Option<&serde_json::Value>,
    ) -> bool {
        match id.and_then(serde_json::Value::as_u64) {
            Some(id) => {
                governor.invalidate(Invalidate::User(id));
                !self.github_origins().is_empty()
            }
            None => self.forget_every_github_repo(governor),
        }
    }

    /// Closes the domains of every connected repository in `repos` at once,
    /// each with the reason `reason` gives for it. Answers whether any was
    /// connected.
    fn close_github_repos(
        &self,
        governor: &GithubGovernor,
        repos: impl IntoIterator<Item = String>,
        reason: impl Fn(&str) -> String,
    ) -> bool {
        let mut closed = false;
        let mut seen = HashSet::new();
        for (_, repo) in self.github_origins() {
            seen.insert(repo_key(&repo));
        }
        let mut done = HashSet::new();
        for repo in repos {
            let key = repo_key(&repo);
            if seen.contains(&key) && done.insert(key) {
                tracing::info!(
                    "GitHub says the App no longer reaches {repo}, so its domain is closed to all but instance admins"
                );
                governor.close(&repo, reason(&repo));
                closed = true;
            }
        }
        closed
    }

    /// The repositories the loaded App client knows installation `id`
    /// covers (none without an id or a loaded App).
    fn github_installation_repositories(&self, id: Option<u64>) -> Vec<String> {
        match (id, self.github_app_slot_settled()) {
            (Some(id), Some(super::github_app::AppSlot::Ready(router))) => {
                router.app().repositories_of_installation(id)
            }
            _ => Vec::new(),
        }
    }

    /// The App client forgets installation `id` (its token and the
    /// repositories it was recorded for), or every installation when the
    /// payload names none. Only an App that is loaded already is told: one
    /// that is not has nothing to forget.
    fn evict_github_installation(&self, id: Option<u64>) {
        if let Some(super::github_app::AppSlot::Ready(router)) = self.github_app_slot_settled() {
            match id {
                Some(id) => router.app().evict_installation(id),
                None => router.app().evict_all(),
            }
        }
    }

    /// A repository was transferred. To another owner it is never followed
    /// (Ruling R32): every domain connected to the old name is closed for
    /// good, until an admin reconnects it. A transfer that keeps the owner's
    /// name is a rename. Answers whether a domain was closed.
    async fn github_repo_transferred(
        &self,
        governor: &GithubGovernor,
        payload: &serde_json::Value,
    ) -> super::Result<bool> {
        let (Some(new), Some(old)) = (repository_of(payload), renamed_from("transferred", payload))
        else {
            return Ok(false);
        };
        let same_owner = old
            .split_once('/')
            .zip(new.split_once('/'))
            .is_some_and(|((a, _), (b, _))| a.eq_ignore_ascii_case(b));
        if same_owner {
            return self
                .follow_github_rename(governor, &old, &new, payload)
                .await;
        }
        if !self.github_repo_connected(&old) {
            return Ok(false);
        }
        tracing::warn!(
            "GitHub says {old} moved to {new}; a move to another owner is not followed, so its domain is closed until an admin connects it again"
        );
        self.record_transferred_from(&old, payload).await;
        governor.close_moved(
            &old,
            format!("the repository moved to {new} on GitHub; disconnect the domain and connect it again"),
        );
        governor.invalidate(Invalidate::Repo(new));
        Ok(true)
    }

    /// The ids recorded in `domain`'s origin state, if any can be read.
    fn recorded_repository_ids(&self, domain: &str) -> Option<RepositoryIds> {
        let dir = self.origin_state_dir(domain).ok()?;
        crystalline_remote::state::OriginState::load(&dir)
            .ok()
            .flatten()?
            .repository_ids
    }

    /// A transfer away from `old`: every domain connected to it that has no
    /// ids recorded yet records the repository's id and the OLD owner's id
    /// (`changes.owner.from.user.id` or `.organization.id`), so the lookup
    /// after a restart sees another owner and keeps the domain closed. This
    /// can only narrow: ids that do not match what GitHub answers close a
    /// domain, never open one. Under each domain's origin lock; a lock held
    /// longer than a few seconds (a pull) leaves the write to a background
    /// task.
    async fn record_transferred_from(&self, old: &str, payload: &serde_json::Value) {
        let repository = payload
            .pointer("/repository/id")
            .and_then(serde_json::Value::as_u64);
        let owner = payload
            .pointer("/changes/owner/from/user/id")
            .or_else(|| payload.pointer("/changes/owner/from/organization/id"))
            .and_then(serde_json::Value::as_u64);
        let (Some(repository), Some(owner)) = (repository, owner) else {
            return;
        };
        let ids = RepositoryIds { repository, owner };
        let key = repo_key(old);
        for (domain, repo) in self.github_origins() {
            if repo_key(&repo) != key {
                continue;
            }
            let Ok(dir) = self.origin_state_dir(&domain) else {
                continue;
            };
            let lock = self.origin_lock(&domain);
            let write = async move {
                let _held = lock.lock().await;
                match write_repository_ids(&dir, ids, false) {
                    Ok(true) => tracing::info!(
                        "the repository ids of the domain {domain} were recorded from the transfer, so it stays closed"
                    ),
                    Ok(false) => {}
                    Err(e) => tracing::warn!(
                        "the repository ids of the domain {domain} could not be recorded from the transfer: {e}"
                    ),
                }
            };
            let task = tokio::spawn(write);
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }
    }

    /// A repository was renamed from `old` to `new` (what the payload says).
    /// Both names are forgotten. The domains connected to `old` are
    /// connected to `new` (one atomic configuration write) only when GitHub
    /// confirms it: asked with the installation token, the repository with
    /// the payload's `repository.id` and the repository `old` leads to
    /// (GitHub redirects a renamed one) are the same id, and GitHub calls it
    /// `new`. A body sent again after the repository moved on finds GitHub
    /// saying something else and moves nothing. Answers false: a rename
    /// does not change who reads the repository.
    async fn follow_github_rename(
        &self,
        governor: &GithubGovernor,
        old: &str,
        new: &str,
        payload: &serde_json::Value,
    ) -> super::Result<bool> {
        let old_key = repo_key(old);
        let connected: Vec<String> = self
            .github_origins()
            .into_iter()
            .filter(|(_, repo)| repo_key(repo) == old_key)
            .map(|(domain, _)| domain)
            .collect();
        if connected.is_empty() || old_key == repo_key(new) {
            governor.invalidate(Invalidate::Repo(new.to_string()));
            return Ok(false);
        }
        let confirmed = self
            .github_confirms_rename(old, new, payload.pointer("/repository/id"))
            .await;
        match confirmed {
            None => {
                tracing::info!(
                    "a delivery says {old} is now {new}, but GitHub does not confirm it, so its domains keep their origin"
                );
            }
            Some(_) if self.read_only => {
                tracing::warn!(
                    "GitHub says {old} is now {new}, but this instance is read-only, so its domains keep the old name; GitHub redirects it"
                );
            }
            Some(live) => {
                // A domain whose recorded repository is another one (its
                // name was reused before) is not this repository's.
                let confirmed_id = payload
                    .pointer("/repository/id")
                    .and_then(serde_json::Value::as_u64);
                let skip: HashSet<String> = connected
                    .iter()
                    .filter(|domain| {
                        self.recorded_repository_ids(domain)
                            .is_some_and(|ids| Some(ids.repository) != confirmed_id)
                    })
                    .cloned()
                    .collect();
                for domain in &skip {
                    tracing::warn!(
                        "GitHub says {old} is now {live}, but the domain {domain} was connected to another repository of that name, so it keeps its origin"
                    );
                }
                let moved = self.move_origin_repo(&old_key, &live, &skip)?;
                for domain in connected
                    .iter()
                    .filter(|d| !moved.contains(d) && !skip.contains(*d))
                {
                    tracing::warn!(
                        "GitHub says {old} is now {live}, but the domain {domain} is not defined in the configuration file, so it keeps the old name; GitHub redirects it"
                    );
                }
            }
        }
        governor.invalidate(Invalidate::Repo(old.to_string()));
        governor.invalidate(Invalidate::Repo(new.to_string()));
        Ok(false)
    }

    /// GitHub's own name for the renamed repository, when GitHub confirms
    /// that `old` is now `new`: `GET /repositories/{id}` and `GET /repos/{old}`
    /// answer the same id and the name `new`. `None` when it does not, when
    /// the payload has no id, when no App is loaded, or when GitHub does not
    /// answer within [`CONFIRM_TIMEOUT`].
    async fn github_confirms_rename(
        &self,
        old: &str,
        new: &str,
        id: Option<&serde_json::Value>,
    ) -> Option<String> {
        let id = id?.as_u64()?;
        let Some(super::github_app::AppSlot::Ready(router)) = self.github_app_slot_settled() else {
            return None;
        };
        let app = router.app().clone();
        let (by_id, by_name) = tokio::time::timeout(CONFIRM_TIMEOUT, async {
            tokio::join!(app.repository_by_id(old, id), app.repository_identity(old))
        })
        .await
        .ok()?;
        let (by_id, by_name) = match (by_id, by_name) {
            (Ok(by_id), Ok(by_name)) => (by_id, by_name),
            (Err(e), _) | (_, Err(e)) => {
                tracing::info!("GitHub could not confirm that {old} is now {new}: {e}");
                return None;
            }
        };
        (by_id.0 == id
            && by_name.0 == id
            && by_id.1.eq_ignore_ascii_case(new)
            && by_name.1.eq_ignore_ascii_case(new))
        .then_some(by_id.1)
    }

    /// Connects every domain of the configuration file whose origin is the
    /// repository `old_key` (lowercased) to `new`, in one write under the
    /// configuration lock, and then records the new name in each domain's
    /// origin state. Answers the domains moved.
    fn move_origin_repo(
        &self,
        old_key: &str,
        new: &str,
        skip: &HashSet<String>,
    ) -> super::Result<Vec<String>> {
        let moved = {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard)?;
            let mut moved = Vec::new();
            for (name, entry) in file.domains.iter_mut() {
                if skip.contains(name) {
                    continue;
                }
                if let Some(origin) = entry.origin.as_mut()
                    && repo_key(&origin.repo) == old_key
                {
                    tracing::info!(
                        "the repository of the domain {name} moved on GitHub from {} to {new}",
                        origin.repo
                    );
                    origin.repo = new.to_string();
                    moved.push(name.clone());
                }
            }
            if moved.is_empty() {
                // Another process (a CLI next to the daemon) may have moved
                // them in the file already: take the file as it is, so this
                // engine does not keep serving the old name.
                let stale = self.config.read().unwrap().domains.values().any(|entry| {
                    entry
                        .origin
                        .as_ref()
                        .is_some_and(|origin| repo_key(&origin.repo) == old_key)
                });
                if stale {
                    let effective = self.overlay.apply(&file);
                    *file_guard = file;
                    *self.config.write().unwrap() = effective;
                }
                return Ok(moved);
            }
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
            moved
        };
        for domain in &moved {
            self.record_origin_repo(domain, new);
        }
        Ok(moved)
    }

    /// Stores `repo` as the repository in `domain`'s origin state, which
    /// `origin status` reports, under the domain's origin lock so a pull in
    /// flight does not write the old name back. In the background: a pull
    /// may hold the lock for a while, and the webhook answers at once. Best
    /// effort; a pull reads its repository from the configuration, never
    /// from here.
    fn record_origin_repo(&self, domain: &str, repo: &str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Ok(dir) = self.origin_state_dir(domain) else {
            return;
        };
        let lock = self.origin_lock(domain);
        let domain = domain.to_string();
        let repo = repo.to_string();
        runtime.spawn(async move {
            let _held = lock.lock().await;
            match crystalline_remote::state::OriginState::load(&dir) {
                Ok(Some(mut state)) if state.repo != repo => {
                    state.repo = repo;
                    if let Err(e) = state.save(&dir) {
                        tracing::warn!(
                            "the new repository name of the domain {domain} could not be stored in its origin state: {e}"
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(
                    "the origin state of the domain {domain} could not be read to store its new repository name: {e}"
                ),
            }
        });
    }

    /// The person GitHub id `id` names revoked the App: the account linked
    /// to that id on this App's host loses its user token and keeps its
    /// link. Nothing happens without a loaded App, an accounts store or a
    /// linked account.
    async fn github_app_revoked_by(&self, id: Option<&serde_json::Value>) -> super::Result<()> {
        let Some(id) = id.and_then(serde_json::Value::as_u64) else {
            return Ok(());
        };
        let host = match self.github_app_slot_settled() {
            Some(super::github_app::AppSlot::Ready(router)) => {
                router.app().credentials().host.clone()
            }
            _ => return Ok(()),
        };
        let Some(access) = self.domain_access.get().cloned() else {
            return Ok(());
        };
        let account = access
            .account_for_github(&host, id)
            .await
            .map_err(|e| super::EngineError::Internal(e.to_string()))?;
        match account {
            Some(account) => self.github_authorization_revoked(&account).await,
            None => Ok(()),
        }
    }
}

/// Stores `ids` in the origin state under `dir`: always when `replace`,
/// else only when none are recorded yet. Answers whether it wrote. A domain
/// without an origin state has nothing to record into.
fn write_repository_ids(
    dir: &std::path::Path,
    ids: RepositoryIds,
    replace: bool,
) -> Result<bool, RemoteError> {
    let Some(mut state) = crystalline_remote::state::OriginState::load(dir)? else {
        return Ok(false);
    };
    if state.repository_ids.is_some() && !replace {
        return Ok(false);
    }
    state.repository_ids = Some(ids);
    state.save(dir)?;
    Ok(true)
}

/// What handling one GitHub App webhook event did, for the surface that
/// received it ([`Engine::apply_github_event`](super::Engine::apply_github_event)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GithubEventOutcome {
    /// Whether who reads a connected domain may have changed, so the
    /// surface runs the Reflection rescan a visibility change runs.
    pub readers_changed: bool,
}

/// The `full_name` (`owner/name`) of a repository object, when it has one.
fn full_name(repository: &serde_json::Value) -> Option<String> {
    let name = repository.get("full_name")?.as_str()?.trim();
    (name.split('/').count() == 2 && !name.starts_with('/') && !name.ends_with('/'))
        .then(|| name.to_string())
}

/// The repository a payload is about (`repository.full_name`).
fn repository_of(payload: &serde_json::Value) -> Option<String> {
    full_name(payload.get("repository")?)
}

/// The installation a payload is about (`installation.id`).
fn installation_of(payload: &serde_json::Value) -> Option<u64> {
    payload.pointer("/installation/id")?.as_u64()
}

/// The `full_name` of every repository in the array `field` of `payload`.
fn repositories_in(payload: &serde_json::Value, field: &str) -> Vec<String> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map(|repos| repos.iter().filter_map(full_name).collect())
        .unwrap_or_default()
}

/// The name a renamed or transferred repository had before: the old name
/// on the same owner (`changes.repository.name.from`) for a rename, or the
/// same name on the old owner (`changes.owner.from.user.login`, or
/// `.organization.login`) for a transfer.
fn renamed_from(action: &str, payload: &serde_json::Value) -> Option<String> {
    let new = repository_of(payload)?;
    let (owner, name) = new.split_once('/')?;
    let text = |pointer: &str| {
        payload
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.contains('/'))
    };
    match action {
        "renamed" => Some(format!(
            "{owner}/{}",
            text("/changes/repository/name/from")?
        )),
        "transferred" => {
            let from = text("/changes/owner/from/user/login")
                .or_else(|| text("/changes/owner/from/organization/login"))?;
            Some(format!("{from}/{name}"))
        }
        _ => None,
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
        /// Internal (GitHub Enterprise): not public, read by the enterprise.
        internal: bool,
        /// Every call on the repository fails with this, while set.
        down: Option<fn() -> RemoteError>,
        not_installed: bool,
        /// Its id and its owner's id, as `GET /repos/{o}/{r}` answers them.
        ids: Option<RepositoryIds>,
        /// Its current name, when GitHub answers another one (renamed).
        full_name: Option<String>,
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

        async fn repository_facts(&self, repo: &str) -> Result<RepositoryFacts, RemoteError> {
            if let Some(e) = self.refusal(repo) {
                return Err(e);
            }
            let repos = lock(&self.repos);
            let found = repos.get(&repo_key(repo));
            Ok(RepositoryFacts {
                visibility: match found {
                    Some(r) if r.public => RepoVisibility::Public,
                    Some(r) if r.internal => RepoVisibility::Internal,
                    _ => RepoVisibility::Private,
                },
                ids: found.and_then(|r| r.ids),
                full_name: Some(
                    found
                        .and_then(|r| r.full_name.clone())
                        .unwrap_or_else(|| repo.to_string()),
                ),
            })
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
        /// How many times a recheck opened a closed domain.
        reopened: AtomicUsize,
        /// The ids recorded for each domain.
        recorded: Mutex<HashMap<String, RepositoryIds>>,
        /// Domains `recorded_ids` leaves out (no origin state).
        left_out: Mutex<HashSet<String>>,
        /// Domains whose origin state cannot be read.
        unreadable: Mutex<HashSet<String>>,
        /// Every rename the governor reported: (old, live).
        renames: Mutex<Vec<(String, String)>>,
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
        fn reopened(&self) {
            self.reopened.fetch_add(1, Ordering::SeqCst);
        }
        fn recorded_ids(&self, repo: &str) -> Vec<(String, Recorded)> {
            let recorded = lock(&self.recorded);
            let left_out = lock(&self.left_out);
            let unreadable = lock(&self.unreadable);
            lock(&self.origins)
                .iter()
                .filter(|(_, r)| repo_key(r) == repo_key(repo))
                .filter(|(domain, _)| !left_out.contains(domain))
                .map(|(domain, _)| {
                    let what = if unreadable.contains(domain) {
                        Recorded::Unreadable
                    } else {
                        recorded
                            .get(domain)
                            .copied()
                            .map_or(Recorded::Nothing, Recorded::Ids)
                    };
                    (domain.clone(), what)
                })
                .collect()
        }
        fn record_ids(&self, domain: &str, ids: RepositoryIds) {
            lock(&self.recorded).insert(domain.to_string(), ids);
        }
        fn renamed(&self, old: &str, live: &str) {
            lock(&self.renames).push((old.to_string(), live.to_string()));
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
            reopened: AtomicUsize::new(0),
            recorded: Mutex::new(HashMap::new()),
            left_out: Mutex::new(HashSet::new()),
            unreadable: Mutex::new(HashSet::new()),
            renames: Mutex::new(Vec::new()),
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

    const IDS: RepositoryIds = RepositoryIds {
        repository: 55,
        owner: 7,
    };

    #[tokio::test(start_paused = true)]
    async fn a_repository_without_recorded_ids_records_them_on_its_first_lookup() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.github.with(REPO, |r| r.ids = Some(IDS));
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(lock(&rig.env.recorded).get(DOMAIN), Some(&IDS));
    }

    #[tokio::test(start_paused = true)]
    async fn a_transferred_repository_is_closed_on_every_lookup() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        lock(&rig.env.recorded).insert(DOMAIN.to_string(), IDS);
        rig.github.with(REPO, |r| {
            r.ids = Some(RepositoryIds {
                repository: 55,
                owner: 8,
            });
            r.full_name = Some("other/handbook".to_string());
        });
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(
            rig.governor.closed()[0].reason,
            "the repository moved to other/handbook on GitHub; disconnect the domain and connect it again"
        );
        // Asked again later, it is decided again, and the same.
        tokio::time::advance(FRESH_FOR).await;
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.github.listings(), 2);
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert!(lock(&rig.env.renames).is_empty());
        assert_eq!(lock(&rig.env.recorded).get(DOMAIN), Some(&IDS));
    }

    #[tokio::test(start_paused = true)]
    async fn a_reused_name_is_closed() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        lock(&rig.env.recorded).insert(DOMAIN.to_string(), IDS);
        rig.github.with(REPO, |r| {
            r.ids = Some(RepositoryIds {
                repository: 56,
                owner: 7,
            })
        });
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(
            rig.governor.closed()[0].reason,
            format!(
                "the name {REPO} now belongs to another repository on GitHub; disconnect the domain and connect it again"
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_rename_within_the_owner_stays_open_and_tells_the_engine() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        lock(&rig.env.recorded).insert(DOMAIN.to_string(), IDS);
        rig.github.with(REPO, |r| {
            r.ids = Some(IDS);
            r.full_name = Some("acme/guide".to_string());
        });
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert!(rig.governor.closed_domains().is_empty());
        assert_eq!(
            *lock(&rig.env.renames),
            vec![(REPO.to_string(), "acme/guide".to_string())]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_domain_left_out_of_the_records_stops_a_rename() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        lock(&rig.env.left_out).insert(DOMAIN.to_string());
        rig.github.with(REPO, |r| {
            r.ids = Some(IDS);
            r.full_name = Some("other/handbook".to_string());
        });
        rig.knowledge("alice").await;
        assert!(lock(&rig.env.renames).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreadable_origin_state_closes_the_domain() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        lock(&rig.env.unreadable).insert(DOMAIN.to_string());
        rig.github.with(REPO, |r| r.ids = Some(IDS));
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(
            rig.governor.closed()[0].reason,
            format!("the origin state of {DOMAIN} cannot be read")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_reconnected_repository_is_asked_at_once_and_opens() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        lock(&rig.env.recorded).insert(DOMAIN.to_string(), IDS);
        let moved = RepositoryIds {
            repository: 55,
            owner: 8,
        };
        rig.github.with(REPO, |r| r.ids = Some(moved));
        rig.knowledge("alice").await;
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        // An admin reconnects: the new ids are recorded and the governor
        // told.
        lock(&rig.env.recorded).insert(DOMAIN.to_string(), moved);
        rig.governor.reconnected(REPO);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert!(rig.governor.closed_domains().is_empty());
    }

    /// Lets the background tasks the governor spawned run.
    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn close_closes_an_open_domain_at_once_with_its_reason() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.governor.close(
            "ACME/Handbook",
            "the repository was deleted on GitHub".into(),
        );
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(
            rig.governor.closed()[0].reason,
            "the repository was deleted on GitHub"
        );
        // A grace of a minute before GitHub is asked, so a GitHub that has
        // not caught up yet does not open it again at once.
        settle().await;
        assert_eq!(rig.github.installation_lookups.load(Ordering::SeqCst), 0);
        tokio::time::advance(CLOSE_GRACE).await;
        rig.governor.governed_domains();
        settle().await;
        assert_eq!(rig.github.installation_lookups.load(Ordering::SeqCst), 1);
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(rig.env.reopened.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_domain_the_recheck_opens_tells_the_engine() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.knowledge("alice").await;
        rig.governor
            .close(REPO, "removed from the App's installation".into());
        tokio::time::advance(CLOSE_GRACE).await;
        rig.governor.governed_domains();
        settle().await;
        assert!(rig.governor.closed_domains().is_empty());
        assert!(rig.governor.closed().is_empty(), "the reason went with it");
        assert_eq!(rig.env.reopened.load(Ordering::SeqCst), 1);
        // Nothing old is served: the next question asks GitHub.
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.github.listings(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_moved_repository_stays_closed_whatever_github_says() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.knowledge("alice").await;
        let reason = "the repository moved to other/handbook on GitHub; disconnect the domain and connect it again";
        rig.governor.close_moved(REPO, reason.into());
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(rig.governor.closed()[0].reason, reason);
        for what in [
            Invalidate::Repo(REPO.to_string()),
            Invalidate::Installation(1),
            Invalidate::All,
        ] {
            rig.governor.invalidate(what);
            tokio::time::advance(FRESH_FOR).await;
            rig.governor.governed_domains();
            settle().await;
            assert!(rig.governor.closed_domains().contains(DOMAIN));
        }
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::None));
        assert_eq!(rig.github.listings(), 1);
        assert_eq!(rig.github.installation_lookups.load(Ordering::SeqCst), 0);
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
                internal: false,
                github_ids: [1, 5].into(),
                listed_ids: [1, 5, 6].into(),
            })
        );
        assert!(rig.governor.repository_public(DOMAIN).await);
        assert_eq!(rig.github.listings(), 1);
    }

    /// An internal repository is read by its enterprise's members, whom the
    /// listing does not name: its readers say so, and it is not public.
    #[tokio::test(start_paused = true)]
    async fn repository_readers_say_when_a_repository_is_internal() {
        let rig = rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Read);
        rig.github.with(REPO, |r| r.internal = true);
        assert_eq!(
            rig.governor.repository_readers(DOMAIN).await,
            Some(RepoReaders {
                public: false,
                internal: true,
                github_ids: [1].into(),
                listed_ids: [1].into(),
            })
        );
        assert!(!rig.governor.repository_public(DOMAIN).await);
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

    /// At start every governed repository is looked up once in the
    /// background: one without the App is closed before anybody asks, and
    /// one with it has a snapshot for the first question.
    #[tokio::test(start_paused = true)]
    async fn the_warm_up_closes_a_repository_without_the_app_before_anyone_asks() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.governor.warm_up();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        assert_eq!(rig.github.listings(), 1);

        let rig = self::rig().await;
        rig.github.grant(REPO, 1, "alice-gh", RepoRole::Write);
        rig.governor.warm_up();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(rig.github.listings(), 1);
        assert_eq!(rig.knowledge("alice").await, level(MemberLevel::Editor));
        assert_eq!(rig.github.listings(), 1, "answered from the warm-up");
        // A second warm-up while the snapshot is fresh asks nothing.
        rig.governor.warm_up();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(rig.github.listings(), 1);
    }

    /// Without a usable App a due re-check cannot ask anybody: it is put
    /// off five minutes instead of being started again on every question.
    #[tokio::test(start_paused = true)]
    async fn a_recheck_without_a_usable_app_waits_five_minutes() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.knowledge("alice").await;
        tokio::time::advance(FRESH_FOR).await;
        *lock(&rig.env.app) = GovernorApp::Unusable;
        rig.governor.governed_domains();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        let next = lock(&rig.governor.state).not_installed[&repo_key(REPO)];
        assert_eq!(next, Instant::now() + FRESH_FOR);
    }

    /// An App that changed and is not usable: the old App's closed set is
    /// dropped, and the base rules apply (R21).
    #[tokio::test(start_paused = true)]
    async fn invalidate_all_without_a_usable_app_drops_the_closed_set() {
        let rig = rig().await;
        rig.github.with(REPO, |r| r.not_installed = true);
        rig.knowledge("alice").await;
        assert!(rig.governor.closed_domains().contains(DOMAIN));
        *lock(&rig.env.app) = GovernorApp::Unusable;
        rig.governor.invalidate(Invalidate::All);
        assert!(rig.governor.closed_domains().is_empty());
        assert!(rig.governor.governed_domains().contains(DOMAIN));
    }
}
