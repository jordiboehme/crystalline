//! Users and server-side sessions for the REST API, in their own small
//! database beside the index.
//!
//! Deliberately separate from the engine's store: credentials are not
//! knowledge, they must survive a `reindex --wipe` that discards the index,
//! and the `crystalline users` CLI has to edit them in another process while
//! the daemon serves. That last point sets the shape here - one connection per
//! [`AuthStore`], every statement its own short autocommit transaction, and a
//! busy timeout so the two writers wait for each other instead of failing.
//!
//! **This database is opened by more than one process at a time, and that is a
//! deliberate deviation from the assumption the index store is built on.**
//! [`crystalline_index::TursoStore`]'s module comment states that "a single
//! [`Connection`] is used from a single task [...] other processes never open
//! the database concurrently, so Turso's young multi-process path is never
//! exercised". That holds for the index, which the daemon alone owns. It does
//! not hold here: `crystalline users ...` opens this file while `serve` has it
//! open, so this module does exercise that path, on purpose.
//!
//! Concurrency therefore has two independent layers, and both are needed:
//!
//! 1. *Across processes*, the CLI and the daemon each have their own
//!    connection. The file is opened with turso's experimental multiprocess
//!    WAL, without which the daemon's open holds an exclusive lock on the
//!    whole file and the CLI cannot open it at all (see `open_database`,
//!    which is where the interesting part of this lives). On top of that,
//!    `PRAGMA busy_timeout` is set on every connection so a writer waits
//!    instead of failing, no transaction spans more than the two statements of
//!    [`AuthStore::remove_user`], and the two multi-statement operations
//!    ([`AuthStore::remove_user`] and [`AuthStore::create_session`]) take
//!    `BEGIN IMMEDIATE` so they serialize against each other rather than
//!    interleaving. Covering test:
//!    `users_add_works_while_another_process_holds_the_auth_db` in the CLI's
//!    `tests/users.rs`, which is the only one that spawns a second process, and
//!    which is `#[cfg(unix)]` because turso 0.7.2 (still so at 0.8.1) has no shared WAL
//!    coordination on the default Windows IO backend (that file says the whole
//!    of it). `two_stores_on_one_file_interleave_writes` below covers the
//!    ordering within one process and nothing about the locking.
//!
//!    Where layer 1 cannot be had, the fallback open is what runs, and a
//!    second process is then refused at open time: `legacy_open_error` turns
//!    that refusal into a message that names the daemon holding the file and
//!    the ways out, instead of turso's byte-range wording.
//! 2. *Within one process*, every method serializes on `AuthStore::guard`,
//!    because turso's [`Connection`] refuses concurrent use outright rather
//!    than queueing. The daemon shares one `AuthStore` across axum handlers, so
//!    two simultaneous logins really are two concurrent calls on one
//!    connection. Covering tests:
//!    `concurrent_sessions_on_one_store_never_fail_spuriously` and
//!    `concurrent_mixed_writes_on_one_store_never_fail_spuriously`.
//!
//! Layer 1 says nothing about layer 2 and vice versa; neither substitutes for
//! the other. If turso's multi-process behavior ever proves unreliable, the
//! layer 1 test is where it will show up first.
//!
//! Two secrets are stored, neither in the clear. Passwords are argon2id at the
//! [`argon2`] crate's own recommended defaults, so the cost parameters follow
//! the crate rather than a number frozen here. Session tokens are 32 random
//! bytes, handed to the client as hex and kept only as their sha256: a stolen
//! copy of this file cannot be replayed as a live session. The CSRF token is
//! not a bearer credential (it is checked against the value the session
//! already proves the caller holds) and is stored as issued.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use sha2::{Digest, Sha256};
use turso::{Builder, Connection, Database, Row, Value};

/// What a user may do. Ordered least to most privileged; the REST layer maps
/// each endpoint to the minimum role it accepts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Sees no engram on any shared domain. What it works on is its
    /// Reflection level and the private domains it is invited into.
    None,
    /// Read only: search, read, browse.
    Viewer,
    /// Everything a viewer may do, plus writing and editing engrams.
    Editor,
    /// Everything an editor may do, plus managing domains and users.
    Admin,
}

impl Role {
    /// The wire and database spelling, which is also what [`serde`] emits.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::None => "none",
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Role::None),
            "viewer" => Ok(Role::Viewer),
            "editor" => Ok(Role::Editor),
            "admin" => Ok(Role::Admin),
            other => Err(anyhow!(
                "unknown role '{other}': expected none, viewer, editor or admin"
            )),
        }
    }
}

/// The role an account provisioned from a single sign-on is created at when
/// `auth.oidc.default_role` is unset.
///
/// One definition for the whole workspace: the settings registry renders it as
/// the key's unset value, and the relying party provisions at it. A promise
/// made in two places is a promise that can drift, and this is the promise the
/// documentation makes to an operator who never sets the key.
pub const DEFAULT_OIDC_ROLE: Role = Role::Viewer;

/// Read a role back out of a database row. An unrecognized value can only come
/// from a hand-edited or corrupted file, so it resolves to the least
/// privileged role rather than failing the whole read: an unreadable row must
/// never fail open.
fn role_from_db(s: &str) -> Role {
    s.parse().unwrap_or(Role::Viewer)
}

/// Fold a supplied user name to the one form this store keys on: trimmed of
/// surrounding whitespace and lowercased. Empty is rejected, and so is any
/// name with whitespace left after trimming - a login name is space-free, the
/// readable form belongs in the display name instead.
///
/// This is enforced here rather than left to callers because the store is the
/// only place every path meets. `name TEXT PRIMARY KEY` byte-compares, so
/// without folding a trusted-header value of `Ada` would provision a second
/// account beside an admin-created `ada` - at the default role, handing back
/// access to someone who had just been disabled or demoted. There is no caller
/// that can be trusted to remember this, so the store does it once for all of
/// them.
///
/// `to_lowercase` is full Unicode case folding, matching the convention
/// `crates/index` already uses for domain and tag names.
///
/// This is the fold every path uses, including the ones merely NAMING an
/// account. A path that CREATES one asks [`normalize_new_account_name`]
/// instead, which additionally refuses the one reserved name.
pub fn normalize_account_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(refuse(
            RefusalKind::InvalidName,
            "a user name cannot be empty".to_string(),
        ));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(refuse(
            RefusalKind::InvalidName,
            "a login name cannot contain whitespace: pick a space-free name \
             and put the readable form in the display name"
                .to_string(),
        ));
    }
    Ok(trimmed.to_lowercase())
}

/// [`normalize_account_name`] for a name that is about to become an ACCOUNT,
/// which is where one name is reserved: [`crate::OWNER_IDENTITY_NAME`],
/// the key the machine owner's own private drafts are written under in every
/// domain that reviews changes.
///
/// An account keyed on it would act inside those drafts - read them, edit them,
/// share them, fold them - in every reviewing domain at once, and the machine
/// owner would be acting inside that account's drafts in the same breath. So
/// the four paths that can bring an account into being ask this instead of the
/// plain fold: an admin creating a login, the first-run admin, a trusted header
/// provisioning one, and an identity provider's first sign-in.
///
/// Only those four. Every other caller is naming an account that already
/// exists - a membership row, a domain's holder, a session, a password check -
/// and the machine owner's own name is a legitimate principal there: it holds
/// drafts, it appears in a fold plan, and a lookup that refused it would refuse
/// the machine owner its own work.
pub fn normalize_new_account_name(name: &str) -> Result<String> {
    let folded = normalize_account_name(name)?;
    if folded == crate::scope::OWNER_IDENTITY_NAME {
        return Err(refuse(
            RefusalKind::InvalidName,
            format!(
                "'{}' is the name this machine's own work is filed under, so it is not a login \
                 anybody can hold: pick another name for this account",
                crate::scope::OWNER_IDENTITY_NAME
            ),
        ));
    }
    Ok(folded)
}

/// Fold one half of an identity key: trimmed, and never empty.
///
/// Deliberately NOT lowercased, unlike an account name. An issuer url and a
/// provider's subject are opaque strings the provider chose, compared byte for
/// byte by every OpenID Connect implementation there is; folding their case
/// here would make two distinct subjects the same person on any provider whose
/// identifiers are case sensitive.
fn identity_value(value: &str, what: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("an identity {what} cannot be empty");
    }
    Ok(trimmed.to_string())
}

/// One account. Carries no password material, so it is safe to hand to a
/// handler and serialize into a response.
#[derive(Clone, Debug, serde::Serialize)]
pub struct User {
    /// The login name and primary key. Also the identity the trusted-header
    /// mode provisions against.
    pub name: String,
    /// Human-readable name for the UI.
    pub display: String,
    /// Optional contact address; never used for login.
    pub email: Option<String>,
    /// What this account may do.
    pub role: Role,
    /// A disabled account keeps its rows but can neither log in nor use an
    /// already-issued session.
    pub disabled: bool,
    /// When this account last resolved a session or arrived through the
    /// trusted header, RFC 3339. Null for an account never seen.
    pub last_seen: Option<String>,
    /// The Reflection level on every shared domain.
    pub reflection: ReflectionLevel,
}

/// One identity an external provider asserts, tied to one account.
///
/// `(issuer, subject)` is the durable key: a username, an address and a
/// display name are all mutable presentation data, and none of them may move
/// an account. An account may hold several links (one per issuer), and a link
/// points at exactly one account.
#[derive(Clone, Debug, serde::Serialize)]
pub struct IdentityLink {
    /// The provider that asserts this identity, as its ID tokens spell it.
    pub issuer: String,
    /// The provider's stable identifier for the person.
    pub subject: String,
    /// When the link was made, RFC 3339.
    pub linked_at: String,
    /// Who made it: the account that linked it, an admin's name, or `jit` for
    /// a link a first sign-in created along with its account.
    pub linked_by: String,
}

/// The GitHub account a Crystalline account is linked to. The numeric
/// `github_id` is the identity (a login can be renamed or recycled); `login`
/// is only a display cache that [`AuthStore::set_github_login`] refreshes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct GithubLink {
    /// The Crystalline account.
    pub account: String,
    /// The GitHub host in its one stored form: lowercase, trimmed, `github.com`
    /// for GitHub.com and the bare Enterprise Server host (no scheme, no
    /// path) otherwise. The same spelling `AppCredentials::host` carries.
    pub host: String,
    /// GitHub's numeric user id on that host.
    pub github_id: u64,
    /// The login last seen for the id, a cache.
    pub login: String,
    /// When the link was made.
    pub linked_at: chrono::DateTime<chrono::Utc>,
    /// False once the stored user token is known to be gone (revoked or
    /// expired); the link itself stays.
    pub token_live: bool,
}

/// The stored form of a GitHub host: trimmed and lowercased.
fn normalize_github_host(host: &str) -> Result<String> {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        bail!("a GitHub host must not be empty");
    }
    Ok(host)
}

const GITHUB_LINK_COLUMNS: &str = "account, host, github_id, login, linked_at, token_live";

fn github_link_from_row(row: &Row) -> GithubLink {
    GithubLink {
        account: cell_text(row, 0).unwrap_or_default(),
        host: cell_text(row, 1).unwrap_or_default(),
        github_id: match row.get_value(2) {
            Ok(Value::Integer(id)) => u64::try_from(id).unwrap_or_default(),
            _ => 0,
        },
        login: cell_text(row, 3).unwrap_or_default(),
        linked_at: cell_text(row, 4)
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
            .map(|at| at.with_timezone(&chrono::Utc))
            .unwrap_or_default(),
        token_live: matches!(row.get_value(5), Ok(Value::Integer(live)) if live != 0),
    }
}

/// What [`AuthStore::provision_linked_user`] records as the linker when a
/// first sign-in creates the account it links.
pub const LINKED_BY_JIT: &str = "jit";

/// What `crystalline users link` records as the linker. The machine operator
/// is not a signed-in account, so there is no name to write: what the row can
/// honestly say is that somebody at the command line did it, which is exactly
/// the distinction the profile card draws between a link a person made for
/// themselves and one an administrator made for them.
pub const LINKED_BY_CLI: &str = "cli";

/// How many suffixed names a provisioning tries before it gives up. Reached
/// only when a thousand accounts already hold every variant of one name, which
/// is a configuration problem rather than a collision.
const MAX_NAME_ATTEMPTS: usize = 1000;

/// What checking a password found, kept apart by how much work each one costs.
///
/// Only the first two run argon2. [`PasswordCheck::NoHash`] returns before any
/// hashing, which is what makes it cheap enough to hear over a network - see
/// [`AuthStore::check_password`].
#[derive(Debug)]
pub enum PasswordCheck {
    /// The password matched this account's hash.
    Verified(User),
    /// There was a hash and the password did not match it.
    Mismatch,
    /// There was no hash to check against: no such account, a disabled one, an
    /// account provisioned without a password, or a name that will not
    /// normalize. Deliberately one variant: the caller must not be able to tell
    /// these apart either, and none of them did any argon2 work.
    NoHash,
}

/// A freshly issued session. The `token` is the only copy in existence that is
/// not hashed - it goes to the client and is never written down here.
#[derive(Clone, Debug)]
pub struct Session {
    /// The session token, 32 random bytes as hex. Set as the session cookie.
    pub token: String,
    /// The CSRF token this session must echo on unsafe requests.
    pub csrf: String,
    /// Unix seconds at which the session stops being accepted.
    pub expires_at: i64,
}

/// What [`AuthStore::ensure_session`] found or did.
///
/// The two are answered differently on the wire: a created session has a token
/// to put in a cookie, a reused one does not, because the stored copy is hashed
/// and the original was handed out once and never kept.
#[derive(Clone, Debug)]
pub enum SessionMint {
    /// The account already held a live session; this is its CSRF token.
    Reused {
        /// The CSRF token that session's requests must echo.
        csrf: String,
    },
    /// The account held none, so one was issued.
    Created(Session),
}

impl SessionMint {
    /// The CSRF token either way, which is what the caller always needs.
    pub fn csrf(&self) -> &str {
        match self {
            SessionMint::Reused { csrf } => csrf,
            SessionMint::Created(session) => &session.csrf,
        }
    }
}

/// A freshly issued (or rotated) MCP token. The `token` is the only copy in
/// existence that is not hashed - it goes to the client once, in the issuance
/// response, and is never written down here.
#[derive(Clone)]
pub struct IssuedMcpToken {
    /// The row id, used to revoke or rotate this token later.
    pub id: i64,
    /// The token itself: [`MCP_TOKEN_PREFIX`] plus 64 hex characters.
    pub token: String,
    /// The caller-chosen label, echoed back so the response is self-describing.
    pub label: String,
}

/// Hand-written rather than derived so `id` and `label` still print (a later
/// task's `tracing::debug!(?issued)` or a failed `assert_eq!` needs those to
/// be useful) while `token` - the only unhashed copy of a live credential -
/// never reaches a log line.
impl std::fmt::Debug for IssuedMcpToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedMcpToken")
            .field("id", &self.id)
            .field("token", &"cmt_[redacted]")
            .field("label", &self.label)
            .finish()
    }
}

/// A freshly minted draft share-link. The `token` is the only copy in
/// existence that is not hashed - it goes back to its author once, in the
/// minting response, so they can hand it on, and is never written down here.
#[derive(Clone)]
pub struct MintedGrant {
    /// The row id, which is what revokes this link later.
    pub id: i64,
    /// The link itself: [`DRAFT_LINK_PREFIX`] plus 64 hex characters.
    pub token: String,
}

/// Hand-written for the reason [`IssuedMcpToken`]'s is: the id still prints,
/// so a `tracing::debug!` or a failed assertion says something useful, while
/// the one unhashed copy of a live credential never reaches a log line.
impl std::fmt::Debug for MintedGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedGrant")
            .field("id", &self.id)
            .field("token", &"dl_[redacted]")
            .finish()
    }
}

/// One share-link on one draft, as the store holds it. Never carries the token:
/// only its sha256 is written, so a listing can say who holds a link and when
/// it was made and can never hand the link itself back out.
#[derive(Clone, Debug, serde::Serialize)]
pub struct OverlayGrant {
    /// The row id, which is what revokes this link.
    pub id: i64,
    /// The domain the drafted engram lives in.
    pub domain: String,
    /// The domain-relative path of the draft this link opens.
    pub path: String,
    /// The account whose draft it is: the actor the overlay entry belongs to.
    pub owner: String,
    /// The account this link bound itself to, or null while nobody has opened
    /// it yet. The first account to redeem it is that account for good.
    pub grantee: Option<String>,
    /// RFC 3339, when the link was minted.
    pub created_at: String,
    /// RFC 3339, when the link stops working on its own, or null for one that
    /// lasts as long as the draft does.
    pub expires_at: Option<String>,
    /// RFC 3339, when its author took it back, or null while it stands.
    pub revoked_at: Option<String>,
}

impl OverlayGrant {
    /// Whether this grant still opens anything as of now: not revoked, and not
    /// past its own window.
    ///
    /// The window is compared by parsing rather than by string order, because
    /// an RFC 3339 instant carries its offset and two spellings of one moment
    /// do not sort. An `expires_at` that cannot be parsed is treated as past:
    /// a grant nobody can date is one nobody should be able to redeem.
    pub(crate) fn is_live(&self) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        match &self.expires_at {
            None => true,
            Some(at) => match chrono::DateTime::parse_from_rfc3339(at) {
                Ok(at) => at > chrono::Utc::now(),
                Err(_) => false,
            },
        }
    }
}

/// A registered OAuth client, as the authorization server stores it. Public
/// clients are the only kind here (`token_endpoint_auth_method: "none"`), so
/// there is no secret in this record and nothing in it needs redacting: a
/// client id is an identifier, and PKCE is what proves the exchange belongs to
/// the browser that started the flow.
#[derive(Clone, Debug)]
pub struct OauthClient {
    /// [`OAUTH_CLIENT_PREFIX`] plus 32 hex characters, the primary key.
    pub client_id: String,
    /// The name the client called itself, shown on the consent screen.
    pub client_name: String,
    /// The client's own page, when it registered one.
    pub client_uri: Option<String>,
    /// Every redirect uri this registration may be sent back to, in the order
    /// it registered them. Never empty.
    pub redirect_uris: Vec<String>,
    /// RFC 3339, when the registration was written.
    pub created_at: String,
    /// RFC 3339, when this registration last started an authorization.
    /// `None` until one does, which is what makes it prunable.
    pub last_used: Option<String>,
}

/// A freshly issued - or freshly rotated - OAuth grant. The two tokens are the
/// only unhashed copies in existence: they go to the client once, in the token
/// endpoint's answer, and only their sha256 is written here.
#[derive(Clone)]
pub struct IssuedOauthGrant {
    /// The grant row's id, which is what revokes it later and what a rotation
    /// keeps: a refresh moves one grant along rather than forking it.
    pub id: i64,
    /// [`OAUTH_ACCESS_PREFIX`] plus 64 hex characters.
    pub access_token: String,
    /// [`OAUTH_REFRESH_PREFIX`] plus 64 hex characters.
    pub refresh_token: String,
    /// Seconds the access token has left, [`OAUTH_ACCESS_TTL_SECS`], reported
    /// to the client as RFC 6749's `expires_in`.
    pub expires_in: u64,
}

/// Hand-written for the reason [`IssuedMcpToken`]'s is: the id still prints,
/// so a `tracing::debug!` or a failed assertion says something useful, while
/// neither live credential can reach a log line.
impl std::fmt::Debug for IssuedOauthGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedOauthGrant")
            .field("id", &self.id)
            .field("access_token", &"coa_[redacted]")
            .field("refresh_token", &"cor_[redacted]")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// A token pair about to be minted: the two unhashed tokens, which leave
/// this store only inside an [`IssuedOauthGrant`].
struct TokenPair {
    access_token: String,
    refresh_token: String,
}

impl TokenPair {
    fn new() -> TokenPair {
        TokenPair {
            access_token: format!("{OAUTH_ACCESS_PREFIX}{}", random_hex()),
            refresh_token: format!("{OAUTH_REFRESH_PREFIX}{}", random_hex()),
        }
    }

    fn issued(&self, id: i64) -> IssuedOauthGrant {
        IssuedOauthGrant {
            id,
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            expires_in: OAUTH_ACCESS_TTL_SECS.unsigned_abs(),
        }
    }
}

/// Whether the audience a refresh asked for (`asked`) is the one the grant
/// holds in column `column` of `row`. Nothing asked always matches. Fails
/// closed: a grant whose audience cannot be read is one whose audience
/// cannot be checked, and answering it with a pair would answer a question
/// this store does not know the answer to.
fn audience_matches(row: &Row, column: usize, asked: Option<&str>) -> Result<bool> {
    let Some(asked) = asked else {
        return Ok(true);
    };
    let Ok(Value::Text(held)) = row.get_value(column) else {
        bail!("an oauth grant row has an unreadable resource");
    };
    Ok(normalize_resource(asked) == normalize_resource(&held))
}

/// What presenting a refresh token did.
///
/// Four outcomes rather than a `Result<Option<_>>`, because two of them are
/// not a failure to find anything: a replay is a fact about the grant that the
/// caller has to act on, and a wrong audience is a live grant that was
/// deliberately left where it was.
pub enum RefreshOutcome {
    /// The grant moved along: both tokens are new, the predecessors are dead.
    Rotated(IssuedOauthGrant),
    /// The token had been rotated less than [`OAUTH_REFRESH_GRACE_SECS`] ago,
    /// and nothing minted from it has been rotated since, so it got a pair of
    /// its own: a parallel session of the same client, or a retry after a
    /// lost answer. The new refresh token is live in its own right, and so is
    /// the one the first rotation handed out; the grant lives on.
    Reissued {
        /// The new pair, exactly as a rotation hands one out.
        grant: IssuedOauthGrant,
        /// Seconds between the token's rotation and this presentation.
        since_rotation: i64,
    },
    /// Nothing matched - an invented token, one at the wrong registration, one
    /// past its window, or one belonging to an account that can no longer sign
    /// in. Deliberately one outcome for all four, the way
    /// [`AuthStore::mcp_token_user`] answers `None` for all of its misses.
    Unknown,
    /// The token had already been rotated away, so a copy of it outlived the
    /// rotation: it came back after [`OAUTH_REFRESH_GRACE_SECS`], or a token
    /// minted from it has itself been rotated since. The grant it belonged to
    /// is revoked by the time this is returned; `grant` is the id it had, for
    /// the log line.
    Replayed {
        /// The id of the grant this replay revoked.
        grant: i64,
        /// Seconds between the token's rotation and this presentation.
        since_rotation: i64,
    },
    /// The token is live and the grant is this client's, but the request asked
    /// for a resource the grant does not hold. Nothing was rotated: handing
    /// back a token whose audience is not the one asked for would tell the
    /// client something untrue about the credential it is holding, and RFC 8707
    /// section 2.2 has an error for exactly this.
    ///
    /// **This is the one outcome here that is not indistinguishable from a
    /// miss, and deliberately so.** It is reachable only from inside the live
    /// lookup, so a caller who gets it has learned that the token it presented
    /// is a live one for that client - and it learned it without rotating
    /// anything, so it can ask again. That is accepted rather than overlooked:
    /// a refresh token is 32 random bytes, so this tells nothing to anybody who
    /// does not already hold one, and the alternative is to answer a client
    /// that made a protocol mistake with a refusal that sends it back through
    /// the whole authorization flow it did not need to repeat. Every other
    /// miss - invented, expired, wrong client, disabled account - stays one
    /// [`RefreshOutcome::Unknown`], and a replay stays indistinguishable from
    /// all of them at the route.
    WrongTarget,
}

/// Hand-written rather than derived, so the rule that no type carrying a token
/// derives `Debug` holds for the wrapper too. `Rotated` prints through
/// [`IssuedOauthGrant`]'s redacting `Debug`, which is where the redaction is.
impl std::fmt::Debug for RefreshOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshOutcome::Rotated(grant) => f.debug_tuple("Rotated").field(grant).finish(),
            RefreshOutcome::Reissued {
                grant,
                since_rotation,
            } => f
                .debug_struct("Reissued")
                .field("grant", grant)
                .field("since_rotation", since_rotation)
                .finish(),
            RefreshOutcome::Unknown => f.write_str("Unknown"),
            RefreshOutcome::WrongTarget => f.write_str("WrongTarget"),
            RefreshOutcome::Replayed {
                grant,
                since_rotation,
            } => f
                .debug_struct("Replayed")
                .field("grant", grant)
                .field("since_rotation", since_rotation)
                .finish(),
        }
    }
}

/// One row of an account's OAuth grant list: which client is connected, since
/// when, and until when it may keep refreshing. Never carries a token - only
/// hashes are stored, so there is nothing to show back.
#[derive(Clone, Debug, serde::Serialize)]
pub struct OauthGrantInfo {
    /// The grant's id, which is what revokes it.
    pub id: i64,
    /// The registration this grant belongs to.
    pub client_id: String,
    /// The name that registration gave for itself, or a stand-in when the
    /// registration is gone.
    pub client_name: String,
    /// The host the client is redirected back to, for a person deciding
    /// whether they recognize this connection.
    pub redirect_host: String,
    /// RFC 3339, when the grant was created.
    pub created_at: String,
    /// RFC 3339, when one of its access tokens last resolved a request.
    pub last_used: Option<String>,
    /// RFC 3339, when the refresh token stops working unless it is rotated
    /// before then. For an ended grant, when it ended.
    pub refresh_expires_at: String,
    /// Set when the grant has ended and this row is only the record of why,
    /// listed for seven days; null for a live grant.
    pub ended: Option<OauthGrantEnded>,
}

/// When and why a grant ended, so a person can tell why a client is gone.
#[derive(Clone, Debug, serde::Serialize)]
pub struct OauthGrantEnded {
    /// RFC 3339, when the grant ended.
    pub at: String,
    /// Why it ended.
    pub reason: OauthGrantEndReason,
}

/// Why a grant ended. Only the endings a person could not otherwise explain
/// are recorded: an expired grant was simply not used for thirty days, and a
/// removed account has nobody left to tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OauthGrantEndReason {
    /// A rotated refresh token came back after the grace window, the replay
    /// rule of a refresh.
    Replayed,
    /// The account itself revoked it, on its Connected clients card.
    Revoked,
    /// The client revoked it with one of its own tokens (RFC 7009), for
    /// example on sign-out.
    ClientRevoked,
}

impl OauthGrantEndReason {
    fn as_str(self) -> &'static str {
        match self {
            OauthGrantEndReason::Replayed => "replayed",
            OauthGrantEndReason::Revoked => "revoked",
            OauthGrantEndReason::ClientRevoked => "client_revoked",
        }
    }

    fn from_db(s: &str) -> Option<OauthGrantEndReason> {
        match s {
            "replayed" => Some(OauthGrantEndReason::Replayed),
            "revoked" => Some(OauthGrantEndReason::Revoked),
            "client_revoked" => Some(OauthGrantEndReason::ClientRevoked),
            _ => None,
        }
    }
}

/// One row of an account's MCP token list, for a management UI or CLI. Never
/// carries the token itself - only the hash is stored, so there is nothing to
/// show back after issuance.
#[derive(Clone, Debug, serde::Serialize)]
pub struct McpTokenInfo {
    /// The row id, used to revoke or rotate this token.
    pub id: i64,
    /// The caller-chosen label.
    pub label: String,
    /// RFC 3339, when this token was issued.
    pub created_at: String,
    /// RFC 3339, when this token last resolved a request. `None` if it has
    /// never been used.
    pub last_used: Option<String>,
}

/// What a member may do on one private domain. Ordered least to most
/// privileged, exactly as [`Role`] is, and deliberately a separate ladder: an
/// account's instance role says what it may do on the installation, this says
/// what it may do on one domain somebody invited it to.
///
/// `Manager` is the level that may invite and change other members' levels. It
/// may not flip the domain back to shared, and it may not hand the domain to
/// someone else: those two stay with the owner (and with an admin), which is
/// what keeps "who can see this at all" a decision the owner made.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberLevel {
    /// Sees no engram of this domain; its Reflection level decides what it
    /// works on.
    None,
    /// Read only: this domain is visible and searchable, nothing more.
    Viewer,
    /// Everything a viewer may do, plus writing and editing its engrams.
    Editor,
    /// Everything an editor may do, plus managing this domain's membership.
    Manager,
}

impl MemberLevel {
    /// The wire and database spelling, which is also what [`serde`] emits.
    pub fn as_str(self) -> &'static str {
        match self {
            MemberLevel::None => "none",
            MemberLevel::Viewer => "viewer",
            MemberLevel::Editor => "editor",
            MemberLevel::Manager => "manager",
        }
    }
}

impl std::fmt::Display for MemberLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemberLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<MemberLevel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(MemberLevel::None),
            "viewer" => Ok(MemberLevel::Viewer),
            "editor" => Ok(MemberLevel::Editor),
            "manager" => Ok(MemberLevel::Manager),
            other => Err(anyhow!(
                "unknown membership level '{other}': expected none, viewer, editor or manager"
            )),
        }
    }
}

/// Read a membership level back out of a database row. Same contract as
/// [`role_from_db`]: an unrecognized value can only come from a hand-edited or
/// corrupted file, so it resolves to the least privileged level rather than
/// failing the read. An unreadable row must never fail open.
fn member_level_from_db(s: &str) -> MemberLevel {
    s.parse().unwrap_or(MemberLevel::Viewer)
}

/// The Reflection level an account holds on every shared domain, or a member
/// on one private domain, beside its knowledge level. It only ever raises
/// what the knowledge level gives: `Default` is "as knowledge",
/// stored as SQL NULL so no sentinel value is written. There is no stored
/// curator: curating follows from writing the engrams.
/// Ordered `Default < Reader < Contributor`, so the higher of two levels is
/// their `max`.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ReflectionLevel {
    /// As the knowledge level gives: a viewer reads and comments, `none`
    /// sees nothing.
    #[default]
    Default,
    /// Reads the Reflection and comments on it.
    Reader,
    /// Also edits reflection text and creates, moves and deletes reflections.
    Contributor,
}

impl ReflectionLevel {
    /// The wire spelling. `Default` is never written to the database.
    pub fn as_str(self) -> &'static str {
        match self {
            ReflectionLevel::Default => "default",
            ReflectionLevel::Reader => "reader",
            ReflectionLevel::Contributor => "contributor",
        }
    }

    /// The column value: NULL for `Default`.
    fn to_db(self) -> Value {
        match self {
            ReflectionLevel::Default => Value::Null,
            other => Value::Text(other.as_str().to_string()),
        }
    }
}

impl std::fmt::Display for ReflectionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReflectionLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<ReflectionLevel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" => Ok(ReflectionLevel::Default),
            "reader" => Ok(ReflectionLevel::Reader),
            "contributor" => Ok(ReflectionLevel::Contributor),
            other => Err(anyhow!(
                "unknown Reflection level '{other}': expected default, reader or contributor"
            )),
        }
    }
}

/// Read a Reflection level back out of a row. NULL is `Default`, and so is a
/// value this build does not know: `Default` never raises anything, so an
/// unreadable cell fails closed.
fn reflection_level_from_db(cell: Option<String>) -> ReflectionLevel {
    cell.and_then(|s| s.parse().ok()).unwrap_or_default()
}

/// The one value the `domain_acl.visibility` column is ever written with.
///
/// The column is an enum of one on purpose: a later release can add a second
/// visibility without a migration. Until one exists, *any* row in `domain_acl`
/// means the domain is private and an absent row means it is shared, so this
/// build never has to guess what an unknown value would have meant - it treats
/// every row as the strictest state it knows, which is the only safe direction
/// for an authorization record.
const VISIBILITY_PRIVATE: &str = "private";

/// The one `domain_member.principal_kind` this release writes or reads.
///
/// Decision 4 of the identity plan keeps the column (and its place in the
/// primary key) so a group principal can be added later without schema
/// surgery. Every statement here filters on it, so the day a `group` row
/// exists it is invisible to the user-principal paths rather than silently
/// resolving as a user of the same name.
const PRINCIPAL_USER: &str = "user";

/// Why the store refused a change, as a value rather than as prose.
///
/// The refusals below are the caller's doing rather than the server's,
/// and the surfaces above have to tell them apart in order to answer with the
/// right status. They used to be told apart by matching substrings of the
/// message, which meant any rewording here silently turned a 409 into a 500 -
/// and, worse, invited a surface to forward the store's own words to somebody
/// who should not have them (an account that does not exist and one that is
/// disabled are the same answer to anybody who cannot read the user list).
///
/// So the classification travels as a type. [`StoreRefusal`] carries the
/// message as its `Display`, unchanged, so `{e:#}` still renders exactly what
/// it always did for a log line or an operator-facing surface, while
/// [`StoreRefusal::kind_of`] gives a caller the decision without reading
/// prose. What each surface then SAYS is its own business: the CLI is the
/// machine operator and prints the detail, the REST membership routes collapse
/// the account arm to one word-for-word answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalKind {
    /// The domain is shared, so it has no membership and no owner.
    NotPrivate,
    /// The principal named owns the domain, and an owner holds no membership
    /// row: the row could only ever say less than the truth.
    OwnerIsNotAMember,
    /// A membership with knowledge `none` and Reflection `default` would grant
    /// nothing at all; removing the member says that.
    EmptyMembership,
    /// The principal names no account, or names one that is disabled. ONE
    /// variant for both on purpose: a surface that cannot read the user list
    /// must not be handed a probe for which of the two it is. Also what the
    /// token verbs raise when the account they are acting FOR is gone, which
    /// the self-service surface answers 401 to: the caller's own account
    /// disappeared between the session resolving and the statement running, and
    /// logging in again is what says so.
    NoSuchAccount,
    /// The `(issuer, subject)` pair is already linked, to this account or to
    /// another one. The message names the account that holds it, for the
    /// operator surfaces; the web surface must answer without it, which is why
    /// the classification is what travels.
    IdentityAlreadyLinked,
    /// The account already holds an identity at this issuer, and one per
    /// issuer is the rule: the one it holds has to go first.
    IssuerAlreadyHeld,
    /// The identity being unlinked is the last way into its account: there is
    /// no password to fall back on and no other identity linked, so removing
    /// it would leave nobody able to sign in. See
    /// [`AuthStore::unlink_identity`].
    LastCredential,
    /// No MCP token with that id belongs to the account that asked. One
    /// variant for "never existed" and "not yours" together, so an id cannot be
    /// probed for through the difference.
    NoSuchToken,
    /// `auth.max_users` is reached, so no further account is provisioned. The
    /// caller cannot act on it and an operator can, which is why it is told
    /// apart from a server fault.
    CapReached,
    /// The name given cannot be a login name at all - empty, or carrying
    /// whitespace. A bad request rather than a fault, on the surfaces that can
    /// be handed one.
    InvalidName,
    /// The GitHub account (host and numeric id) is already linked to another
    /// Crystalline account. See [`AuthStore::link_github`].
    GithubIdTaken,
}

/// A refused membership change: [`RefusalKind`] plus the sentence the store
/// would have printed.
#[derive(Debug)]
pub struct StoreRefusal {
    kind: RefusalKind,
    message: String,
}

impl StoreRefusal {
    /// What kind of refusal this is.
    pub fn kind(&self) -> RefusalKind {
        self.kind
    }

    /// The kind of membership refusal inside `error`, if it is one.
    ///
    /// Walks the whole source chain rather than downcasting the outermost
    /// error, so a caller that added its own context on the way up does not
    /// hide the classification.
    pub fn kind_of(error: &anyhow::Error) -> Option<RefusalKind> {
        error
            .chain()
            .find_map(|e| e.downcast_ref::<StoreRefusal>())
            .map(|refusal| refusal.kind)
    }
}

impl std::fmt::Display for StoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StoreRefusal {}

/// Build one, as the error type the store's refusing statements return.
fn refuse(kind: RefusalKind, message: String) -> anyhow::Error {
    anyhow::Error::new(StoreRefusal { kind, message })
}

/// What every surface says when an unlink would take away the last way into an
/// account: what would happen, and the one command that gives the account a
/// second way in first.
///
/// One sentence, written here, forwarded verbatim by the REST route and by the
/// CLI. The account it names is always the caller's own on the web surface and
/// the one an operator typed on the command line, so naming it is not a probe
/// for anybody.
fn last_credential_message(user: &str) -> String {
    format!(
        "this is the only way into account '{user}': it has no password, so unlinking its last \
         identity would leave nobody able to sign in - give it a password first with \
         `crystalline users passwd {user}`"
    )
}

/// The visibility record of one private domain. A row exists only for a domain
/// somebody made private; an absent row is the default, shared visibility,
/// which is why turning privacy off deletes the row rather than rewriting it.
///
/// There is no `visibility` field: see `VISIBILITY_PRIVATE` for why the
/// column exists anyway.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainAcl {
    /// The domain name, stored exactly as the caller registered it (trimmed,
    /// never case folded: the engine keys its domain map on the literal name,
    /// so folding here would conflate two distinct registrations).
    pub domain: String,
    /// The account that owns this domain: the login name, folded by
    /// [`normalize_account_name`] like every other account reference.
    pub owner: String,
}

/// What [`AuthStore::set_domain_visibility`] did, for a caller that has to say
/// so.
///
/// Two answers rather than a bare `()`, because "it was already private" and
/// "it is private now" are the same end state reached from different places and
/// a person deserves to be told which one they are looking at - especially
/// since the second one names an owner the caller chose and the first one names
/// the owner it already had.
#[derive(Clone, Debug, PartialEq)]
pub enum VisibilityWrite {
    /// The visibility record was written: the domain is now what was asked for.
    Written,
    /// The domain was already private and was left exactly as it stood, with
    /// the owner named here and every membership row intact.
    AlreadyPrivate {
        /// The owner it already has, which is not necessarily the account the
        /// caller named.
        owner: String,
    },
}

/// One membership row: who was invited to a private domain, at what level, by
/// whom and when.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DomainMember {
    /// The member's login name, folded by [`normalize_account_name`].
    pub principal: String,
    /// What this member may do here.
    pub level: MemberLevel,
    /// The Reflection level on this domain.
    pub reflection: ReflectionLevel,
    /// Who added or last changed this row. An audit field, stored as given:
    /// it is usually a login name but may name a non-account actor, the same
    /// latitude the identity-link plan gives `linked_by`.
    pub added_by: String,
    /// RFC 3339, when this row was last written.
    pub added_at: String,
}

/// One per-domain Reflection collaborator: an account given the Reflection
/// reader or contributor level on one domain, beside whatever its engram
/// access gives it there. A grant only raises the Reflection right; it never
/// opens an engram.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ReflectionGrant {
    /// The account's login name, folded by [`normalize_account_name`].
    pub account: String,
    /// `reader` or `contributor`, never `default`.
    pub level: ReflectionLevel,
    /// Who added or last changed this grant. An audit field, stored as given.
    pub added_by: String,
    /// RFC 3339, when this grant was last written.
    pub added_at: String,
}

/// Fold a supplied domain name to the form this store keys on: trimmed, and
/// otherwise left exactly as given.
///
/// Deliberately *not* lowercased, which is where this parts company with
/// [`normalize_account_name`]. A domain name is a key in the engine's own domain map
/// (`Engine::domain_entry` does an exact `HashMap` lookup), so `Lab` and `lab`
/// are two different registrations there; folding them together here would let
/// a privacy record written for one hide the other, or - worse - let a lookup
/// for one miss the record protecting it.
fn normalize_domain(domain: &str) -> Result<String> {
    let trimmed = domain.trim();
    if trimmed.is_empty() {
        bail!("a domain name cannot be empty");
    }
    Ok(trimmed.to_string())
}

/// The users and sessions database. Open one per process that needs it: the
/// daemon holds one for the lifetime of `serve`, the `crystalline users` CLI
/// opens one for the length of a single command.
///
/// Safe to share across tasks (`Arc<AuthStore>` in the daemon): every method
/// takes `&self` and serializes its database work internally. See `guard`.
pub struct AuthStore {
    // Retained so the connection stays valid for as long as the store does.
    _db: Database,
    conn: Connection,
    /// Serializes database access within this process.
    ///
    /// turso's [`Connection`] refuses concurrent use outright - a second
    /// caller does not queue behind the busy timeout, it fails immediately
    /// ("concurrent use forbidden", and for the transactional methods "cannot
    /// start a transaction within a transaction"). The daemon holds one
    /// `AuthStore` for the lifetime of `serve`, so two simultaneous logins are
    /// two concurrent [`AuthStore::create_session`] calls on this one
    /// connection: without this lock they would spuriously fail.
    ///
    /// A [`tokio::sync::Mutex`] rather than a connection per call. Auth traffic
    /// is a handful of requests around login, so contention is irrelevant,
    /// while a connection per call would pay turso's open cost and re-apply
    /// `PRAGMA busy_timeout` on every password check, and would multiply this
    /// process's handles on one small file for no gain. It is a `tokio` mutex
    /// rather than a `std` one because it is held across `await`.
    ///
    /// This is the *in-process* half of the concurrency story only. Serializing
    /// here says nothing about the `crystalline users` CLI in another process;
    /// that is what `BEGIN IMMEDIATE` and the busy timeout are for. Both halves
    /// are needed and neither substitutes for the other.
    ///
    /// Argon2 hashing is deliberately kept *outside* the lock: it is tens of
    /// milliseconds of CPU with no database access, and holding the lock across
    /// it would serialize every login behind every other login.
    guard: tokio::sync::Mutex<()>,
}

/// Create the tables on first open. `IF NOT EXISTS` throughout, so this is the
/// same statement on an existing file. `account_write_domain` holds the write
/// lists (`crystalline users writes`): no rows for an account means it has no
/// list. `domain_reflection_grant` holds the per-domain Reflection
/// collaborators ([`AuthStore::set_reflection_grant`]), on any domain kept as
/// files, shared or private. `github_delivery` holds the GitHub App webhook
/// deliveries of the last day ([`AuthStore::remember_delivery`]), so a
/// replayed one does no work. `oauth_grant_token` holds every OAuth grant's
/// token pairs, live and rotated ([`AuthStore::refresh_oauth_grant_at`]
/// explains the rules); the token columns of `oauth_grants` are kept only so
/// an older binary still opens the file, and hold placeholders (see
/// `AuthStore::move_oauth_tokens`). `oauth_grant_ended` records why a grant
/// ended, for the Connected clients card ([`AuthStore::list_ended_oauth_grants`]).
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS users (
    name TEXT PRIMARY KEY,
    display TEXT NOT NULL,
    email TEXT,
    role TEXT NOT NULL,
    reflection TEXT,
    pass_hash TEXT,
    disabled INTEGER NOT NULL DEFAULT 0,
    last_seen_at TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    user_name TEXT NOT NULL,
    csrf TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user_name ON sessions (user_name);
CREATE INDEX IF NOT EXISTS sessions_expires_at ON sessions (expires_at);
CREATE TABLE IF NOT EXISTS mcp_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used TEXT
);
CREATE INDEX IF NOT EXISTS mcp_tokens_user ON mcp_tokens (user);
CREATE TABLE IF NOT EXISTS domain_acl (
    domain TEXT PRIMARY KEY NOT NULL,
    visibility TEXT NOT NULL,
    owner TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS domain_member (
    domain TEXT NOT NULL,
    principal_kind TEXT NOT NULL DEFAULT 'user',
    principal TEXT NOT NULL,
    level TEXT NOT NULL,
    reflection TEXT,
    added_by TEXT NOT NULL,
    added_at TEXT NOT NULL,
    PRIMARY KEY (domain, principal_kind, principal)
);
CREATE INDEX IF NOT EXISTS domain_member_principal
    ON domain_member (principal_kind, principal);
CREATE TABLE IF NOT EXISTS identity_link (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    user TEXT NOT NULL,
    linked_at TEXT NOT NULL,
    linked_by TEXT NOT NULL,
    PRIMARY KEY (issuer, subject)
);
CREATE INDEX IF NOT EXISTS identity_link_user ON identity_link (user);
CREATE UNIQUE INDEX IF NOT EXISTS identity_link_issuer_user
    ON identity_link (issuer, user);
CREATE TABLE IF NOT EXISTS github_link (
    account TEXT PRIMARY KEY NOT NULL, host TEXT NOT NULL, github_id INTEGER NOT NULL,
    login TEXT NOT NULL, linked_at TEXT NOT NULL, token_live INTEGER NOT NULL DEFAULT 1);
CREATE UNIQUE INDEX IF NOT EXISTS github_link_host_id ON github_link (host, github_id);
CREATE TABLE IF NOT EXISTS oauth_clients (
    client_id TEXT PRIMARY KEY,
    client_name TEXT NOT NULL,
    client_uri TEXT,
    redirect_uris TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used TEXT
);
CREATE TABLE IF NOT EXISTS oauth_grants (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user TEXT NOT NULL,
    client_id TEXT NOT NULL,
    resource TEXT NOT NULL,
    access_hash TEXT NOT NULL UNIQUE,
    access_expires_at INTEGER NOT NULL,
    refresh_hash TEXT NOT NULL UNIQUE,
    previous_refresh_hash TEXT,
    refresh_expires_at INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    last_used TEXT
);
CREATE INDEX IF NOT EXISTS oauth_grants_user ON oauth_grants (user);
CREATE INDEX IF NOT EXISTS oauth_grants_client ON oauth_grants (client_id);
CREATE INDEX IF NOT EXISTS oauth_grants_previous_refresh
    ON oauth_grants (previous_refresh_hash);
CREATE TABLE IF NOT EXISTS oauth_grant_token (
    refresh_hash TEXT PRIMARY KEY,
    grant_id INTEGER NOT NULL,
    access_hash TEXT UNIQUE,
    access_expires_at INTEGER NOT NULL,
    refresh_expires_at INTEGER NOT NULL,
    issued_at INTEGER NOT NULL,
    rotated_at INTEGER,
    rotated_from TEXT
);
CREATE INDEX IF NOT EXISTS oauth_grant_token_grant ON oauth_grant_token (grant_id);
CREATE INDEX IF NOT EXISTS oauth_grant_token_from ON oauth_grant_token (rotated_from);
CREATE TABLE IF NOT EXISTS oauth_grant_ended (
    grant_id INTEGER PRIMARY KEY,
    user TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_name TEXT NOT NULL,
    redirect_host TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used TEXT,
    ended_at INTEGER NOT NULL,
    reason TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS oauth_grant_ended_user ON oauth_grant_ended (user, ended_at);
CREATE TABLE IF NOT EXISTS overlay_grant (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain TEXT NOT NULL,
    path TEXT NOT NULL,
    owner TEXT NOT NULL,
    grantee TEXT,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS overlay_grant_draft
    ON overlay_grant (domain, owner, path);
CREATE INDEX IF NOT EXISTS overlay_grant_grantee
    ON overlay_grant (grantee, domain, path);
CREATE TABLE IF NOT EXISTS account_write_domain (
    account TEXT NOT NULL,
    pattern TEXT NOT NULL,
    PRIMARY KEY (account, pattern)
);
CREATE TABLE IF NOT EXISTS domain_reflection_grant (
    domain TEXT NOT NULL, account TEXT NOT NULL, level TEXT NOT NULL,
    added_by TEXT NOT NULL, added_at TEXT NOT NULL, PRIMARY KEY (domain, account));
CREATE TABLE IF NOT EXISTS github_delivery (id TEXT PRIMARY KEY, received_at INTEGER NOT NULL);
";

/// How long a GitHub webhook delivery id is remembered, one day in seconds:
/// a replay inside it does no work (see [`AuthStore::remember_delivery`]).
pub const DELIVERY_KEPT_SECS: i64 = 86_400;

/// The sentence [`AuthStore::link_github`] refuses a GitHub account that is
/// already linked to another Crystalline account with
/// ([`RefusalKind::GithubIdTaken`]). Public so a surface can recognise the
/// refusal by it and answer 409 with these words.
pub const GITHUB_ID_TAKEN: &str = "this GitHub account is already linked to another Crystalline account; an admin can unlink it there";

/// Prefix every MCP token is minted with, so a token is recognizable at a
/// glance and distinct from a session cookie or a CSRF value. The remainder is
/// 64 lowercase hex characters, 32 bytes from the OS CSPRNG.
pub const MCP_TOKEN_PREFIX: &str = "cmt_";

/// Prefix on every OAuth access token, so the MCP gate can tell one from an
/// MCP token by looking at it and route it to the right lookup. The remainder
/// is 64 lowercase hex characters, 32 bytes from the OS CSPRNG, exactly as an
/// MCP token's is.
pub const OAUTH_ACCESS_PREFIX: &str = "coa_";

/// Prefix on every OAuth refresh token. A separate prefix from the access
/// token's on purpose: the two are presented at different endpoints, and a
/// client that sends the wrong one gets a refusal rather than a lookup that
/// happens to miss.
pub const OAUTH_REFRESH_PREFIX: &str = "cor_";

/// Prefix on every registered client id. Not a credential - a public client
/// authorizes with nothing but this id and PKCE - so its random half is 32 hex
/// characters rather than 64.
pub const OAUTH_CLIENT_PREFIX: &str = "coc_";

/// Prefix on every draft share-link. The remainder is 64 lowercase hex
/// characters, 32 bytes from the OS CSPRNG, exactly as an MCP token's is: a
/// link that lets somebody read one of another person's unshared drafts is a
/// credential, and it is sized like one.
///
/// Its own prefix rather than a reused one, for the reason the OAuth pair have
/// two: these are presented at a different endpoint from every other token
/// here, and a link pasted into the wrong box should be refused rather than
/// looked up somewhere it could never match.
pub const DRAFT_LINK_PREFIX: &str = "dl_";

/// How long an OAuth access token lives, in seconds. One hour: long enough
/// that a session is not a stream of refreshes, short enough that a leaked
/// token stops working on its own, and the number the token endpoint reports
/// as `expires_in`.
pub const OAUTH_ACCESS_TTL_SECS: i64 = 3600;

/// How long an OAuth refresh token lives, in seconds. Thirty days, restarted
/// at every rotation, so a client used at all keeps working and one abandoned
/// for a month has to ask its person again.
pub const OAUTH_REFRESH_TTL_SECS: i64 = 30 * 24 * 3600;

/// How long a rotated refresh token may still be presented, in seconds, and
/// get a pair of its own instead of revoking its grant.
///
/// Several sessions of one MCP client often share one stored credential, and
/// when the access token runs out they all refresh with the same refresh
/// token at nearly the same moment. A client that lost the answer to a
/// refresh retries with the token it still holds. Without a window both look
/// exactly like a stolen token coming back, and the grant was revoked about
/// once a day. Inside the window such a token gets its own fresh pair (see
/// [`RefreshOutcome::Reissued`]); after it the token is a replay and revokes
/// the grant, as before. Sixty seconds is far longer than the gap between
/// two parallel refreshes or a retry, and short enough that a copy of a token
/// found later still trips the replay rule.
pub const OAUTH_REFRESH_GRACE_SECS: i64 = 60;

/// How many live refresh tokens one grant may hold at once. Every answer
/// inside [`OAUTH_REFRESH_GRACE_SECS`] adds one, so a client whose parallel
/// sessions come and go would collect them without end. At every new token
/// the oldest live ones past this number are deleted outright: presenting
/// one later is an unknown token (the session signs in again), never a
/// replay that would end the whole grant for the others.
pub const OAUTH_LIVE_TOKENS_KEPT: i64 = 8;

/// How many rotated refresh tokens one grant remembers, newest first. They
/// are what makes a replay detectable, also two or more rotations back; a
/// token older than these is forgotten and reads as unknown.
pub const OAUTH_RETIRED_TOKENS_KEPT: i64 = 8;

/// How long the record of an ended grant is listed on the account's
/// Connected clients card, in seconds: seven days, long enough that a person
/// who finds a client signed out on Monday still sees why.
pub const OAUTH_ENDED_KEPT_SECS: i64 = 7 * 24 * 3600;

/// How many ended-grant records one account keeps at most, newest first.
pub const OAUTH_ENDED_KEPT_PER_ACCOUNT: i64 = 20;

/// The prefix of the inert values `oauth_grants.access_hash` and
/// `oauth_grants.refresh_hash` hold since the tokens moved to
/// `oauth_grant_token`. The two columns are `NOT NULL UNIQUE` in the table an
/// older binary creates, so they cannot simply be left empty; a token hash is
/// lowercase hex and never holds a `:`, so no presented token can match one.
const OAUTH_PLACEHOLDER_PREFIX: &str = "none:";

/// How long a registration that HAS been authorized is kept once it stops
/// being used, in seconds. Thirty days from the last authorization a person
/// allowed, so somebody's client between connections keeps its identity while
/// one nobody has used in a month is swept by
/// [`AuthStore::prune_oauth_clients`]. A registration that holds a grant is
/// never pruned, however old it is.
pub const OAUTH_CLIENT_UNUSED_SECS: i64 = 30 * 24 * 3600;

/// How long a registration nobody has authorized is kept, in seconds. One
/// hour, and the short clock is the point.
///
/// The two ages measure different things. A row nobody has authorized is the
/// residue of a client that registered and walked away - or of an anonymous
/// caller filling the table, which is the only way
/// `crystalline_rest::MAX_OAUTH_CLIENTS` is ever reached, since registration is
/// the one write nobody has to authenticate for. Keeping such a row for thirty
/// days is exactly what would make that filling stick: the table would stay
/// full for a month and every real client would be refused for as long. An
/// hour is far longer than any client needs between registering and sending
/// its person to consent, and short enough that a filled table drains by
/// itself.
///
/// **"Authorized" means a person allowed it, not that a request arrived.** The
/// authorize leg carries no identity and passes by construction for whoever
/// owns the registration, so a clock that started on a request rather than on
/// a decision would cost an attacker one extra call per row and buy back the
/// whole thirty days. [`AuthStore::touch_oauth_client`] is where that line is
/// drawn.
pub const OAUTH_CLIENT_UNAUTHORIZED_SECS: i64 = 3600;

/// What [`AuthStore::list_oauth_grants`] shows where a client name should be,
/// for a grant whose registration is gone. The grant stays listed - and
/// therefore revocable - rather than disappearing from the one screen that can
/// stop it.
const GONE_OAUTH_CLIENT: &str = "a client that is no longer registered";

/// What [`redirect_host`] shows for a redirect uri that names no host at all.
/// A `javascript:` or `data:` uri parses, carries no host and is entirely
/// attacker-chosen text; the consent screen and the grant card label that
/// field as the address a person is asked to recognize, so a uri with no host
/// says so rather than being allowed to write the line itself.
const NO_REDIRECT_HOST: &str = "an address with no host";

/// The columns every user read selects, in the order [`user_from_row`] decodes.
const USER_COLUMNS: &str = "name, display, email, role, disabled, last_seen_at, reflection";

/// [`USER_COLUMNS`] qualified for the session join, where `users` is aliased
/// `u`. Same columns in the same order, so [`user_from_row`] decodes both.
const USER_COLUMNS_JOINED: &str =
    "u.name, u.display, u.email, u.role, u.disabled, u.last_seen_at, u.reflection";

/// A `WHERE` fragment that is true unless the row it matches is the last
/// *enabled* admin: either this account is not an enabled admin, or another
/// enabled admin exists. `?1` is the target name, so it composes with every
/// statement here that already keys on `?1`.
///
/// Losing the last enabled admin locks the installation out of its own user
/// management, with no way back short of editing the database by hand. The
/// check lives in the statement rather than in the `crystalline users` CLI on
/// purpose: a read-then-write in the CLI would race a second invocation (or
/// the daemon's own writes) and two concurrent demotions could each see the
/// other admin and both go through. As part of the statement it is decided by
/// the one writer that holds the write lock.
///
/// A *disabled* admin deliberately does not count as a remaining admin: it
/// cannot log in, so it is not a way back in.
const NOT_LAST_ADMIN: &str = "(role <> 'admin' OR disabled <> 0 OR EXISTS (
         SELECT 1 FROM users other
         WHERE other.name <> ?1 AND other.role = 'admin' AND other.disabled = 0
     ))";

/// What a refused edit tells the operator. `{verb}` is filled per call site.
fn last_admin_error(verb: &str, name: &str) -> anyhow::Error {
    anyhow!(
        "refusing to {verb} the last admin ('{name}'): \
         add or enable another admin first"
    )
}

/// Open the database file itself, in the one mode that lets a second process
/// open it at the same time.
///
/// turso's default open path takes a whole-file, process-scoped, *exclusive*
/// advisory lock on the database file (`IO::lock_file`, non-blocking, no
/// retry), so while `serve` holds this file open a second process cannot even
/// get to a statement: `crystalline users add` fails at open time with a
/// locking error, and neither the busy timeout nor `BEGIN IMMEDIATE` ever gets
/// a say, because both act after the open. That is the whole bug this flag
/// fixes.
///
/// `experimental_multiprocess_wal(true)` (turso 0.7.2, unchanged at 0.8.1) replaces that lock with
/// a shared coordination file beside the database (`web-auth.db-tshm`): the
/// open adds `OpenFlags::NoLock` instead of locking the file and writers
/// coordinate through that mapping, so the busy timeout finally does the
/// waiting it was always meant to do. The rest is read out of the turso 0.7.2
/// and turso_core 0.7.2 sources, re-read at 0.8.1 (`Database::effective_open_flags_for_path`,
/// `open_with_flags_async`) and, where noted, checked against a running
/// daemon:
///
/// * The two modes are exclusive per file, but nothing enforces that on the
///   path this takes. turso rejects a mixed open in both directions
///   (`reject_live_multiprocess_wal_for_legacy_open` and
///   `reject_live_legacy_wal_for_multiprocess_open`) only in the *synchronous*
///   `Database::open_file_with_flags`; `open_with_flags_async`, which is what
///   `Builder::build` reaches, applies the flags and skips both probes.
///   Checked, not deduced: with a daemon built without this flag holding the
///   file and a build carrying it running `users add`, the write goes through
///   with no complaint, and so does the reverse pairing. This is the only path
///   that opens `web-auth.db`, so one build never disagrees with itself, and
///   an upgrade is the one thing that puts two builds on one file. **Restart
///   the daemon after upgrading the binary, before editing accounts**: two
///   builds that open this file differently would coordinate through different
///   indexes (an in-process one against the `-tshm` mapping) on one WAL.
///   Nothing here can detect that, which is why it is written down here and in
///   `docs/deployment.md` instead. No shipped Crystalline has ever had a
///   `web-auth.db`, though, so the only way to be on the wrong side of this is
///   to run pre-release builds of both halves.
/// * The flag is a no-op, keeping legacy behavior, where turso_core's
///   `host_shared_wal` cfg is off, which is every 32-bit target. There the
///   old failure mode remains.
/// * It is refused outright, rather than ignored, on a memory-like path and
///   on network filesystems (NFS, SMB/CIFS, AFS, Ceph, GFS2, Lustre, 9p and
///   friends). This half *is* on the async path. A state directory on a
///   network share is unusual but real, and failing to open at all would be a
///   worse regression than the bug being fixed, so that one case falls back to
///   a legacy open below.
async fn open_database(path: &Path) -> Result<Database> {
    let name = path.to_string_lossy().to_string();
    let multiprocess = Builder::new_local(&name)
        .experimental_multiprocess_wal(true)
        .build()
        .await;
    let err = match multiprocess {
        Ok(db) => return Ok(db),
        Err(err) if is_multiprocess_unsupported(&err) => err,
        Err(err) => {
            return Err(anyhow::Error::new(err))
                .with_context(|| format!("opening auth database {}", path.display()));
        }
    };
    tracing::warn!(
        error = %err,
        path = %path.display(),
        "this platform does not support cross-process access to the auth database; \
         `crystalline users` will fail while the daemon is running"
    );
    Builder::new_local(&name)
        .build()
        .await
        .map_err(|err| legacy_open_error(err, path))
}

/// Word a failed *fallback* open, the one that runs after multiprocess WAL
/// turned out to be unavailable here.
///
/// One of its failures is not a defect but the predicted consequence of that
/// fallback: with no shared WAL coordination the open takes an exclusive,
/// process-scoped lock on the file, so whichever of the daemon and the CLI
/// opens second is refused at open time. That is today's situation on Windows,
/// where turso 0.7.2's (and 0.8.1's) default IO backend reports no shared WAL coordination
/// (only the off-by-default `experimental_win_iocp` backend does), and it is
/// also what a state directory on a network filesystem gets. Turso words it as
/// a byte-range lock failure, which is true and useless: what the person at the
/// terminal needs is what holds the file and which two ways out exist. Every
/// other failure keeps the plain context, so a corrupt file is never reported
/// as a running daemon.
///
/// Kept as a pure function of the error so it is testable on any OS, not only
/// on the one platform that can produce the lock failure. Covering tests:
/// `a_locked_fallback_open_says_what_holds_the_database` and
/// `a_fallback_open_that_fails_for_another_reason_keeps_the_plain_context`.
fn legacy_open_error(err: turso::Error, path: &Path) -> anyhow::Error {
    if is_locked_by_another_process(&err) {
        anyhow::Error::new(err).context(format!(
            "the auth database {} is held by a running daemon and this platform cannot share it: \
             manage accounts in the web UI, or stop the daemon (`crystalline ctl shutdown`) and \
             try again",
            path.display()
        ))
    } else {
        anyhow::Error::new(err).context(format!("opening auth database {}", path.display()))
    }
}

/// Whether an open failed because another process already holds the file.
///
/// Matched on the message for the same reason as
/// [`is_multiprocess_unsupported`]: turso flattens `LimboError` into the
/// catch-all `turso::Error::Error(String)`, so the `Locking error:` prefix that
/// `LimboError::LockingError`'s `Display` writes is the only thing left to
/// match on.
fn is_locked_by_another_process(err: &turso::Error) -> bool {
    err.to_string().contains("Locking error")
}

/// Whether this open failed because multiprocess WAL cannot be had here at
/// all, as opposed to any other reason an open can fail.
///
/// Matched on the message because turso flattens `LimboError::InvalidArgument`
/// into the catch-all `turso::Error::Error(String)` (see `turso_sdk_kit`'s
/// `From<LimboError> for TursoError`), so the message is the only thing that
/// survives. The three messages it has to catch all end in "is not supported
/// ..." and all begin with the same prefix, which is what is matched. A
/// locking error is deliberately *not* matched: that one means another process
/// holds the file in the other mode, and retrying without the flag would only
/// swap a clear message for a bare lock failure.
fn is_multiprocess_unsupported(err: &turso::Error) -> bool {
    err.to_string()
        .contains("experimental multiprocess WAL is not supported")
}

impl AuthStore {
    /// Open (creating if absent) the auth database at `path`, applying the
    /// schema. The parent directory is created too, so a first run against a
    /// fresh state directory works.
    pub async fn open(path: &Path) -> Result<AuthStore> {
        // Inside an app package a new database file (or its log) under
        // AppData would land in the package's private copy: refused there.
        crystalline_core::config::check_write(path)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let db = open_database(path).await?;
        let conn = db.connect().context("connecting to the auth database")?;
        // The CLI and the daemon write the same file. Wait rather than fail;
        // every transaction here is a single short statement.
        conn.execute("PRAGMA busy_timeout = 5000", ())
            .await
            .context("setting the auth database busy timeout")?;
        // Since turso 0.8.0 `execute_batch` takes exclusive use of the
        // connection and fails with Misuse "connection is busy with another
        // operation" while any `Rows` on it is still alive; none is open here.
        conn.execute_batch(SCHEMA)
            .await
            .context("creating the auth database schema")?;
        ensure_column(&conn, "users", "last_seen_at TEXT").await?;
        // 0.26.0: the Reflection level beside the knowledge level, NULL for
        // "as knowledge". Additive, so an older binary still opens the file
        // (it names its columns and never reads these two).
        ensure_column(&conn, "users", "reflection TEXT").await?;
        ensure_column(&conn, "domain_member", "reflection TEXT").await?;
        let store = AuthStore {
            _db: db,
            conn,
            guard: tokio::sync::Mutex::new(()),
        };
        store.move_oauth_tokens().await?;
        Ok(store)
    }

    /// Move every grant's tokens out of `oauth_grants` into
    /// `oauth_grant_token` (0.32.1). Runs at every open and changes nothing
    /// once every grant is moved, the same contract as the idempotent
    /// `SCHEMA`: there is no version counter to consult.
    ///
    /// A grant still holding a real hash in `refresh_hash` (anything that is
    /// not an [`OAUTH_PLACEHOLDER_PREFIX`] value) gets its current pair as a
    /// live token row, and its `previous_refresh_hash`, when there is one, as
    /// a rotated row. That rotation happened exactly when the current access
    /// token was minted, `access_expires_at - OAUTH_ACCESS_TTL_SECS`, so the
    /// grace window is judged on the true time. Then the grant's own columns
    /// get placeholders. The same pass picks up a grant an older binary
    /// created after a downgrade.
    ///
    /// The columns are kept rather than dropped on purpose: the table and its
    /// `previous_refresh_hash` index stay what an older binary's `SCHEMA`
    /// expects, so a downgraded binary still opens the file. Its OAuth lookups
    /// then match nothing and the clients sign in again, which is a safe way
    /// to fail.
    ///
    /// One `BEGIN IMMEDIATE`, because the daemon and the CLI open this file
    /// concurrently and only one of them may move a grant.
    async fn move_oauth_tokens(&self) -> Result<()> {
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .context("moving oauth tokens to their own table")?;
        let result = async {
            let placeholder = format!("{OAUTH_PLACEHOLDER_PREFIX}%");
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO oauth_grant_token
                         (refresh_hash, grant_id, access_hash, access_expires_at,
                          refresh_expires_at, issued_at, rotated_at, rotated_from)
                     SELECT refresh_hash, id, access_hash, access_expires_at,
                            refresh_expires_at, access_expires_at - ?2, NULL,
                            previous_refresh_hash
                     FROM oauth_grants WHERE refresh_hash NOT LIKE ?1",
                    vec![
                        Value::Text(placeholder.clone()),
                        Value::Integer(OAUTH_ACCESS_TTL_SECS),
                    ],
                )
                .await
                .context("moving live oauth tokens")?;
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO oauth_grant_token
                         (refresh_hash, grant_id, access_hash, access_expires_at,
                          refresh_expires_at, issued_at, rotated_at, rotated_from)
                     SELECT previous_refresh_hash, id, NULL, 0, 0, 0,
                            access_expires_at - ?2, NULL
                     FROM oauth_grants
                     WHERE refresh_hash NOT LIKE ?1 AND previous_refresh_hash IS NOT NULL",
                    vec![
                        Value::Text(placeholder.clone()),
                        Value::Integer(OAUTH_ACCESS_TTL_SECS),
                    ],
                )
                .await
                .context("moving rotated oauth tokens")?;
            self.conn
                .execute(
                    "UPDATE oauth_grants
                     SET access_hash = ?2 || 'access:' || id,
                         refresh_hash = ?2 || 'refresh:' || id,
                         access_expires_at = 0,
                         previous_refresh_hash = NULL
                     WHERE refresh_hash NOT LIKE ?1",
                    vec![
                        Value::Text(placeholder),
                        Value::Text(OAUTH_PLACEHOLDER_PREFIX.to_string()),
                    ],
                )
                .await
                .context("clearing moved oauth token columns")?;
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Add an account with a password. The name is folded by
    /// [`normalize_account_name`], so `Ada` and `ada` are the same account. Errors if
    /// the name is already taken; the primary key is the guard, so two racing
    /// writers cannot both win.
    pub async fn add_user(
        &self,
        name: &str,
        display: &str,
        email: Option<&str>,
        role: Role,
        password: &str,
    ) -> Result<()> {
        self.add_user_with_reflection(
            name,
            display,
            email,
            role,
            ReflectionLevel::Default,
            password,
        )
        .await
    }

    /// [`AuthStore::add_user`] with the account's Reflection level on every
    /// shared domain, written in the same statement as the account, so an
    /// account never exists without the level it was created with.
    pub async fn add_user_with_reflection(
        &self,
        name: &str,
        display: &str,
        email: Option<&str>,
        role: Role,
        reflection: ReflectionLevel,
        password: &str,
    ) -> Result<()> {
        let name = normalize_new_account_name(name)?;
        // Hash before taking the lock: argon2 is CPU, not database.
        let hash = hash_password(password).await?;
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "INSERT INTO users
                     (name, display, email, role, reflection, pass_hash, disabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
                vec![
                    Value::Text(name.clone()),
                    Value::Text(display.to_string()),
                    match email {
                        Some(e) => Value::Text(e.to_string()),
                        None => Value::Null,
                    },
                    Value::Text(role.as_str().to_string()),
                    reflection.to_db(),
                    Value::Text(hash),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .with_context(|| format!("adding user '{name}'"))?;
        Ok(())
    }

    /// How many accounts exist. Zero is what opens the first-run setup path,
    /// and it is what `GET /auth/me` reports as `needs_setup`.
    pub async fn user_count(&self) -> Result<usize> {
        let _guard = self.guard.lock().await;
        Ok(
            match self
                .query_first("SELECT COUNT(*) FROM users", vec![])
                .await?
                .map(|row| row.get_value(0))
            {
                Some(Ok(Value::Integer(n))) => n as usize,
                _ => 0,
            },
        )
    }

    /// Create the first admin, and only into an empty table. Returns whether
    /// this call is the one that created it.
    ///
    /// The zero-check and the insert are ONE statement, which is the whole
    /// point: this is the only claim on the first-account slot that holds
    /// across processes. `crystalline users add` opens this same file from
    /// another process while the daemon serves, so no mutex in the daemon can
    /// serialize against it - and a read-then-write here would let a setup
    /// request and a CLI add both see an empty table and both go through, the
    /// second of them silently creating an admin nobody asked for. As part of
    /// the statement, the `WHERE NOT EXISTS` is decided by whichever writer
    /// holds the write lock, exactly like `NOT_LAST_ADMIN`.
    ///
    /// The name is folded by [`normalize_account_name`] like every other path, and a
    /// name that will not fold is refused before anything is written, so a typo
    /// does not consume the one slot there is. The password is hashed outside
    /// the lock, as in [`AuthStore::add_user`]: argon2 is CPU, not database.
    pub async fn add_first_admin(&self, name: &str, display: &str, password: &str) -> Result<bool> {
        let name = normalize_new_account_name(name)?;
        // Hash before taking the lock: argon2 is CPU, not database.
        let hash = hash_password(password).await?;
        let _guard = self.guard.lock().await;
        let written = self
            .conn
            .execute(
                "INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
                 SELECT ?1, ?2, NULL, 'admin', ?3, 0, ?4
                 WHERE NOT EXISTS (SELECT 1 FROM users)",
                vec![
                    Value::Text(name.clone()),
                    Value::Text(display.to_string()),
                    Value::Text(hash),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .with_context(|| format!("creating the first admin '{name}'"))?;
        Ok(written > 0)
    }

    /// Check a password. `None` covers every way this can fail to produce a
    /// login: unknown name, wrong password, a disabled account, an account
    /// with no password at all (one provisioned by [`AuthStore::ensure_user`]),
    /// and a name that will not normalize. They are deliberately
    /// indistinguishable, so a caller cannot leak which one it was.
    ///
    /// A caller whose *timing* is observable wants
    /// [`AuthStore::check_password`] instead: collapsing the outcomes to
    /// `None` here hides which of them happened from the value, not from the
    /// clock.
    pub async fn verify_password(&self, name: &str, password: &str) -> Result<Option<User>> {
        Ok(match self.check_password(name, password).await? {
            PasswordCheck::Verified(user) => Some(user),
            PasswordCheck::Mismatch | PasswordCheck::NoHash => None,
        })
    }

    /// Check a password, saying which of the three outcomes it was.
    ///
    /// The split exists for one reason: exactly one of them, [`NoHash`], does
    /// no argon2 work, and a caller that answers over the network has to make
    /// up the difference itself or leak the existence of an account through how
    /// fast it answers. See `rest::auth::authenticate`, which pairs
    /// [`NoHash`] with [`dummy_verify`] so every login attempt costs one
    /// verification whatever the outcome.
    ///
    /// [`NoHash`]: PasswordCheck::NoHash
    pub async fn check_password(&self, name: &str, password: &str) -> Result<PasswordCheck> {
        let Ok(name) = normalize_account_name(name) else {
            return Ok(PasswordCheck::NoHash);
        };
        // Scoped so the lock is released before the argon2 verify below.
        let row = {
            let _guard = self.guard.lock().await;
            self.query_first(
                &format!("SELECT {USER_COLUMNS}, pass_hash FROM users WHERE name = ?1"),
                vec![Value::Text(name)],
            )
            .await?
        };
        let Some(row) = row else {
            return Ok(PasswordCheck::NoHash);
        };
        let user = user_from_row(&row);
        if user.disabled {
            return Ok(PasswordCheck::NoHash);
        }
        let Some(hash) = cell_text(&row, 7) else {
            return Ok(PasswordCheck::NoHash);
        };
        if verify_hash(hash, password.to_string()).await? {
            Ok(PasswordCheck::Verified(user))
        } else {
            Ok(PasswordCheck::Mismatch)
        }
    }

    /// Replace an account's password and revoke every session it holds. Errors
    /// if the account does not exist, so a mistyped name on the CLI is reported
    /// rather than silently ignored.
    ///
    /// The revocation is the point rather than a courtesy. A password is reset
    /// because the old one is no longer trusted, and a session issued under it
    /// never presents a password again: without this, whoever holds a cookie
    /// minted before the reset keeps the account for the rest of the session's
    /// life, which is exactly the person a reset is meant to evict. Both
    /// statements are one `BEGIN IMMEDIATE` transaction, so no session from
    /// before the change can survive it, and a refused change (an account that
    /// is not there) revokes nothing.
    pub async fn set_password(&self, name: &str, password: &str) -> Result<()> {
        let name = normalize_account_name(name)?;
        // Hash before taking the lock: argon2 is CPU, not database.
        let hash = hash_password(password).await?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        let result = async {
            let changed = self
                .conn
                .execute(
                    "UPDATE users SET pass_hash = ?2 WHERE name = ?1",
                    vec![Value::Text(name.clone()), Value::Text(hash)],
                )
                .await
                .with_context(|| format!("updating user '{name}'"))?;
            if changed == 0 {
                bail!("no such user: '{name}'");
            }
            self.delete_sessions_of(&name).await
        }
        .await;
        self.finish(result).await
    }

    /// Change an account's role. Demoting the last enabled admin is refused
    /// (see `NOT_LAST_ADMIN`); promoting anyone never is, so the `?2 =
    /// 'admin'` arm short-circuits the guard.
    pub async fn set_role(&self, name: &str, role: Role) -> Result<()> {
        self.update_guarded(
            &format!(
                "UPDATE users SET role = ?2 WHERE name = ?1 AND (?2 = 'admin' OR {NOT_LAST_ADMIN})"
            ),
            name,
            Value::Text(role.as_str().to_string()),
            "demote",
        )
        .await
    }

    /// [`AuthStore::set_role`] without the last-admin guard. The CLI's
    /// `users demote --force` recovery path; HTTP never calls it, so the
    /// installation cannot be locked out over the network - only deliberately,
    /// on the machine that holds this file.
    pub async fn set_role_force(&self, name: &str, role: Role) -> Result<()> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE users SET role = ?2 WHERE name = ?1",
                vec![
                    Value::Text(name.clone()),
                    Value::Text(role.as_str().to_string()),
                ],
            )
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        Ok(())
    }

    /// Set or clear an account's display name. Clearing (None, or a value that
    /// trims to nothing) resets it to the folded login name, so a row is never
    /// nameless: the column is NOT NULL and the UI always has something to
    /// print. No last-admin guard applies - a display name changes nothing
    /// about what the account may do.
    pub async fn set_display(&self, name: &str, display: Option<&str>) -> Result<()> {
        let name = normalize_account_name(name)?;
        let display = display
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| name.clone());
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE users SET display = ?2 WHERE name = ?1",
                vec![Value::Text(name.clone()), Value::Text(display)],
            )
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        Ok(())
    }

    /// Disable or re-enable an account. Disabling deletes every session it
    /// holds, in the same transaction as the flag.
    ///
    /// [`AuthStore::session_user`] also refuses a disabled account's sessions
    /// at read time, and that check stays: it is what makes the effect
    /// immediate for a session another process is already holding open. It is
    /// not enough on its own, though, because it only hides the rows while the
    /// flag is set - re-enabling the account would hand every cookie from
    /// before the disabling back. Deleting them means disabling is a
    /// revocation, which is what an operator disabling a compromised account
    /// is asking for, and re-enabling starts from no sessions at all.
    ///
    /// Disabling the last enabled admin is refused (see `NOT_LAST_ADMIN`);
    /// re-enabling never is, so the `?2 = 0` arm short-circuits the guard. A
    /// refused disabling rolls back, sessions included.
    pub async fn set_disabled(&self, name: &str, disabled: bool) -> Result<()> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        let result = async {
            let changed = self
                .conn
                .execute(
                    &format!(
                        "UPDATE users SET disabled = ?2 \
                         WHERE name = ?1 AND (?2 = 0 OR {NOT_LAST_ADMIN})"
                    ),
                    vec![
                        Value::Text(name.clone()),
                        Value::Integer(i64::from(disabled)),
                    ],
                )
                .await
                .with_context(|| format!("updating user '{name}'"))?;
            if changed == 0 {
                // Zero rows means the account is not there, or that disabling
                // it would have left no enabled admin. The probe is inside the
                // transaction, so it sees exactly what the update saw.
                let exists = self
                    .query_first(
                        "SELECT 1 FROM users WHERE name = ?1",
                        vec![Value::Text(name.clone())],
                    )
                    .await?;
                if exists.is_some() {
                    return Err(last_admin_error("disable", &name));
                }
                bail!("no such user: '{name}'");
            }
            if disabled {
                self.delete_sessions_of(&name).await?;
            }
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Delete an account and everything keyed on its name: its sessions, its
    /// MCP tokens, its private-domain memberships and its write list. Errors
    /// if there is no such account.
    ///
    /// The deletes are one `BEGIN IMMEDIATE` transaction, sessions first.
    /// Both details are load-bearing, because a session row that outlives its
    /// account is not merely garbage: `session_user` resolves a token by
    /// joining `sessions` to `users`, so once a new account claims the freed
    /// name, the old holder's token starts resolving to it. `users remove ada`
    /// followed by `users add ada` would hand the new account to whoever still
    /// held the old cookie.
    ///
    /// Deleting sessions first means a crash between the statements leaves
    /// sessions gone and the account present - recoverable by retrying, and
    /// safe in the meantime. The transaction closes the remaining window
    /// against a concurrent [`AuthStore::create_session`] in the daemon, which
    /// takes `BEGIN IMMEDIATE` too and therefore cannot land an insert between
    /// these two statements.
    ///
    /// Removing the last enabled admin is refused (see `NOT_LAST_ADMIN`).
    /// The guard rides on the `DELETE` itself, inside the same transaction, so
    /// two concurrent removals cannot both observe the other admin and both
    /// succeed. A refusal rolls the session delete back with everything else.
    pub async fn remove_user(&self, name: &str) -> Result<()> {
        let name = normalize_account_name(name)?;
        let key = vec![Value::Text(name.clone())];
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("removing user '{name}'"))?;
        let result = async {
            self.delete_sessions_of(&name).await?;
            self.delete_mcp_tokens_of(&name).await?;
            self.delete_memberships_of(&name).await?;
            self.delete_reflection_grants_of(&name).await?;
            self.delete_identity_links_of(&name).await?;
            self.delete_github_link_of(&name).await?;
            self.delete_oauth_grants_of(&name).await?;
            self.delete_write_list_of(&name).await?;
            self.delete_overlay_grants_held_by(&name).await?;
            self.disown_domains_of(&name).await?;
            let changed = self
                .conn
                .execute(
                    &format!("DELETE FROM users WHERE name = ?1 AND {NOT_LAST_ADMIN}"),
                    key.clone(),
                )
                .await
                .with_context(|| format!("removing user '{name}'"))?;
            if changed == 0 {
                // Zero rows means one of two things, and the operator needs to
                // be told which. The probe is inside the transaction, so it
                // sees exactly what the delete saw.
                let exists = self
                    .query_first("SELECT 1 FROM users WHERE name = ?1", key)
                    .await?;
                if exists.is_some() {
                    return Err(last_admin_error("remove", &name));
                }
                bail!("no such user: '{name}'");
            }
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// [`AuthStore::remove_user`] without the last-admin guard, for
    /// `users remove --force`. Sessions, tokens, memberships and its write
    /// list still go first, in the same `BEGIN IMMEDIATE` transaction, for
    /// the resurrection reasons the guarded remove documents.
    pub async fn remove_user_force(&self, name: &str) -> Result<()> {
        let name = normalize_account_name(name)?;
        let key = vec![Value::Text(name.clone())];
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("removing user '{name}'"))?;
        let result = async {
            self.delete_sessions_of(&name).await?;
            self.delete_mcp_tokens_of(&name).await?;
            self.delete_memberships_of(&name).await?;
            self.delete_reflection_grants_of(&name).await?;
            self.delete_identity_links_of(&name).await?;
            self.delete_github_link_of(&name).await?;
            self.delete_oauth_grants_of(&name).await?;
            self.delete_write_list_of(&name).await?;
            self.delete_overlay_grants_held_by(&name).await?;
            self.disown_domains_of(&name).await?;
            let changed = self
                .conn
                .execute("DELETE FROM users WHERE name = ?1", key)
                .await
                .with_context(|| format!("removing user '{name}'"))?;
            if changed == 0 {
                bail!("no such user: '{name}'");
            }
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// One account by name, or `None` when there is none. The name is folded
    /// by [`normalize_account_name`] like every other lookup, so `Ada` finds `ada`.
    ///
    /// A pure existence-and-details read, with none of
    /// [`AuthStore::session_user`]'s stamping: the caller is asking whether a
    /// name is an account, not resolving a credential, so nothing about the
    /// account changes. What it exists for is telling a mistyped name apart
    /// from a real account with nothing in it - `crystalline users mcp-token
    /// ghsot --list` must say "no such user" rather than "holds no tokens".
    pub async fn user(&self, name: &str) -> Result<Option<User>> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        Ok(self
            .query_first(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
                vec![Value::Text(name)],
            )
            .await?
            .map(|row| user_from_row(&row)))
    }

    /// The name [`AuthStore::provision_linked_user`] would give a new account
    /// wanting `desired` right now.
    pub async fn free_name_from(&self, desired: &str) -> Result<String> {
        let base = normalize_new_account_name(desired)?;
        let _guard = self.guard.lock().await;
        self.free_account_name(&base).await
    }

    /// When the account under `name` was made, or `None` when the name holds no
    /// account. Accounts cannot be renamed, so this tells two accounts under one
    /// name apart: something that began for one of them can ask, before it
    /// writes, whether the name still holds that account.
    pub async fn account_born(&self, name: &str) -> Result<Option<String>> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        Ok(self
            .query_first(
                "SELECT created_at FROM users WHERE name = ?1",
                vec![Value::Text(name)],
            )
            .await?
            .and_then(|row| cell_text(&row, 0)))
    }

    /// Every account, by name. Names sort byte-wise, which is the ordering
    /// contract the rest of the workspace's text columns use.
    pub async fn list_users(&self) -> Result<Vec<User>> {
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                &format!("SELECT {USER_COLUMNS} FROM users ORDER BY name"),
                (),
            )
            .await
            .context("listing users")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.context("listing users")? {
            out.push(user_from_row(&row));
        }
        Ok(out)
    }

    /// The names of the accounts that have no password (a token-only account,
    /// or one a trusted header or a sign-on provisioned), so `users list` can
    /// say so. Kept apart from [`User`], which crosses the REST API.
    pub async fn passwordless_users(&self) -> Result<std::collections::HashSet<String>> {
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query("SELECT name FROM users WHERE pass_hash IS NULL", ())
            .await
            .context("listing accounts without a password")?;
        let mut out = std::collections::HashSet::new();
        while let Some(row) = rows
            .next()
            .await
            .context("listing accounts without a password")?
        {
            if let Some(name) = cell_text(&row, 0) {
                out.insert(name);
            }
        }
        Ok(out)
    }

    /// Return the account for `name`, creating a passwordless one at `role` if
    /// it is absent. This is the trusted-header path: an upstream proxy has
    /// already authenticated the request, so there is no password to store and
    /// none is ever accepted for such an account.
    ///
    /// `role` applies at creation only. A later [`AuthStore::set_role`] by an
    /// admin sticks instead of being reverted on the account's next request.
    ///
    /// The name is folded by [`normalize_account_name`], which matters most here: a
    /// header value of `Ada` must resolve to the existing `ada` rather than
    /// mint a second account at the default role, which would silently undo a
    /// disable or a demotion. The display name keeps the casing as sent.
    ///
    /// `cap` bounds how many accounts this call may *mint*: an account that
    /// already exists always resolves, whatever the current count is relative
    /// to `cap`, and only bringing a new one into existence is refused once
    /// the count has reached it. This is the trusted-header mitigation - a
    /// proxy misconfiguration (a header carrying a session id, say) must not
    /// mint one account per request forever. The check-then-insert runs under
    /// this process's `guard`, so two calls in this process cannot both slip
    /// past it; the cross-process window (a `crystalline users add` racing it
    /// in another process) can overshoot the cap by at most the number of
    /// racing writers, which is acceptable for a mitigation whose job is
    /// stopping *unbounded* minting, not enforcing an exact ceiling.
    pub async fn ensure_user(&self, name: &str, role: Role, cap: usize) -> Result<User> {
        Ok(self.ensure_user_created(name, role, cap).await?.0)
    }

    /// [`AuthStore::ensure_user`], and whether this call made the account, so
    /// the caller can do what a new account needs once.
    pub async fn ensure_user_created(
        &self,
        name: &str,
        role: Role,
        cap: usize,
    ) -> Result<(User, bool)> {
        let display = name.trim().to_string();
        let name = normalize_new_account_name(name)?;
        let _guard = self.guard.lock().await;
        let exists = self
            .query_first(
                "SELECT 1 FROM users WHERE name = ?1",
                vec![Value::Text(name.clone())],
            )
            .await?
            .is_some();
        if !exists {
            let count = match self
                .query_first("SELECT COUNT(*) FROM users", vec![])
                .await?
                .map(|row| row.get_value(0))
            {
                Some(Ok(Value::Integer(n))) => n as usize,
                _ => 0,
            };
            if count >= cap {
                return Err(refuse(
                    RefusalKind::CapReached,
                    format!(
                        "refusing to provision '{name}': the account cap is reached \
                         (auth.max_users = {cap}). Remove unused accounts or raise the cap"
                    ),
                ));
            }
        }
        let inserted = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO users
                     (name, display, email, role, pass_hash, disabled, created_at)
                 VALUES (?1, ?2, NULL, ?3, NULL, 0, ?4)",
                vec![
                    Value::Text(name.clone()),
                    Value::Text(display),
                    Value::Text(role.as_str().to_string()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .with_context(|| format!("provisioning user '{name}'"))?;
        self.conn
            .execute(
                "UPDATE users SET last_seen_at = ?2 WHERE name = ?1",
                vec![
                    Value::Text(name.clone()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .context("stamping last_seen_at")?;
        let user = self
            .query_first(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
                vec![Value::Text(name.clone())],
            )
            .await?
            .map(|row| user_from_row(&row))
            .ok_or_else(|| anyhow!("user '{name}' vanished right after being provisioned"))?;
        Ok((user, inserted > 0))
    }

    /// The account an external provider's `(issuer, subject)` pair names, or
    /// `None` when this instance has never seen that pair.
    ///
    /// The one lookup a sign-on does. It deliberately cannot be asked to find
    /// an account by address or by username: matching either of those would be
    /// the silent takeover this design refuses, and an API that cannot express
    /// it cannot grow it by accident.
    ///
    /// The read drops any link whose account no longer exists, the same sweep
    /// [`AuthStore::session_user`] and [`AuthStore::mcp_token_user`] apply to
    /// their own tables. `identity_link` carries no foreign key, and both
    /// removal paths delete the links inside the transaction that deletes the
    /// account, so nothing here is supposed to write one; but a link is a
    /// credential, and an orphaned one is exactly what would be inherited by
    /// the next account to take that login name - a stranger's provider
    /// identity signing into somebody else's account. Clearing it on sight
    /// makes that unrecoverable rather than dormant.
    pub async fn linked_user(&self, issuer: &str, subject: &str) -> Result<Option<User>> {
        let issuer = identity_value(issuer, "issuer")?;
        let subject = identity_value(subject, "subject")?;
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM identity_link
                 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.name = identity_link.user)",
                (),
            )
            .await
            .context("pruning orphaned identity links")?;
        Ok(self
            .query_first(
                &format!(
                    "SELECT {USER_COLUMNS_JOINED} FROM identity_link l
                     JOIN users u ON u.name = l.user
                     WHERE l.issuer = ?1 AND l.subject = ?2"
                ),
                vec![Value::Text(issuer), Value::Text(subject)],
            )
            .await?
            .map(|row| user_from_row(&row)))
    }

    /// Tie an external identity to an existing account. An explicit act, which
    /// is the whole policy: a sign-on never lands in an account it was not
    /// linked to, and linking is done by the person who is signed in or by an
    /// admin.
    ///
    /// Refused when the pair is already linked (to any account, this one
    /// included), when the account already holds an identity at this issuer,
    /// and when the name is nobody or a disabled account. All three checks run
    /// inside the same `BEGIN IMMEDIATE` transaction as the insert, so what
    /// they saw is what the insert sees.
    pub async fn link_identity(
        &self,
        issuer: &str,
        subject: &str,
        user: &str,
        linked_by: &str,
    ) -> Result<()> {
        let issuer = identity_value(issuer, "issuer")?;
        let subject = identity_value(subject, "subject")?;
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("linking an identity to user '{user}'"))?;
        let result = async {
            self.require_live_user(&user).await?;
            self.insert_link(&issuer, &subject, &user, linked_by).await
        }
        .await;
        self.finish(result).await
    }

    /// Drop the identity `user` holds at `issuer`. `true` when there was one.
    ///
    /// Keyed on `(issuer, user)` rather than on the subject: the person
    /// unlinking knows which provider they want gone, and by construction they
    /// hold at most one identity there.
    ///
    /// Refused with [`RefusalKind::LastCredential`] when the link being removed
    /// is the account's last way in - no password hash and no other identity -
    /// because an account provisioned by a first sign-on has no password by
    /// construction, so unlinking it would leave a live account nobody can
    /// reach. The rule lives here rather than in each surface, so the profile
    /// card, the REST route and the CLI cannot disagree about it. The one way
    /// past it is [`AuthStore::unlink_identity_force`].
    pub async fn unlink_identity(&self, issuer: &str, user: &str) -> Result<bool> {
        self.unlink(issuer, user, false).await
    }

    /// [`AuthStore::unlink_identity`] with the last-way-in guard bypassed, for
    /// the operator repairing an account whose provider re-issued its subject:
    /// the stale identity has to go before the new one can be linked, and the
    /// account is genuinely unreachable in between. Never reachable from the
    /// web surface, where the person doing it would be stranding themselves.
    pub async fn unlink_identity_force(&self, issuer: &str, user: &str) -> Result<bool> {
        self.unlink(issuer, user, true).await
    }

    /// The two above. The read of what else the account has to sign in with
    /// and the delete share one `BEGIN IMMEDIATE`: two concurrent unlinks of
    /// an account's two identities would otherwise both see a second way in
    /// and leave none.
    async fn unlink(&self, issuer: &str, user: &str, force: bool) -> Result<bool> {
        let issuer = identity_value(issuer, "issuer")?;
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("unlinking an identity from user '{user}'"))?;
        let result = async {
            if !force
                && self.link_at(&issuer, &user).await?
                && self.link_count(&user).await? <= 1
                && !self.password_present(&user).await?
            {
                return Err(refuse(
                    RefusalKind::LastCredential,
                    last_credential_message(&user),
                ));
            }
            let changed = self
                .conn
                .execute(
                    "DELETE FROM identity_link WHERE issuer = ?1 AND user = ?2",
                    vec![Value::Text(issuer.clone()), Value::Text(user.to_string())],
                )
                .await
                .with_context(|| format!("unlinking an identity from user '{user}'"))?;
            Ok(changed > 0)
        }
        .await;
        self.finish(result).await
    }

    /// Whether `user` has a local password to sign in with.
    ///
    /// A read of its own rather than a byproduct of
    /// [`AuthStore::check_password`], whose [`PasswordCheck::NoHash`]
    /// deliberately collapses "no hash" with "no such account": here the two
    /// have to be one answer for a different reason - a name that is nobody
    /// has no password either - but the caller is asking about an account it
    /// already holds, and about whether unlinking would strand it.
    pub async fn has_password(&self, name: &str) -> Result<bool> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        self.password_present(&name).await
    }

    /// Every identity linked to one account, by issuer.
    pub async fn identity_links(&self, user: &str) -> Result<Vec<IdentityLink>> {
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT issuer, subject, linked_at, linked_by FROM identity_link
                 WHERE user = ?1 ORDER BY issuer",
                vec![Value::Text(user.clone())],
            )
            .await
            .with_context(|| format!("reading the identity links of user '{user}'"))?;
        let mut links = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("reading the identity links of user '{user}'"))?
        {
            links.push(IdentityLink {
                issuer: cell_text(&row, 0).unwrap_or_default(),
                subject: cell_text(&row, 1).unwrap_or_default(),
                linked_at: cell_text(&row, 2).unwrap_or_default(),
                linked_by: cell_text(&row, 3).unwrap_or_default(),
            });
        }
        Ok(links)
    }

    /// Link `account` to the GitHub account `github_id` on `host`. Relinking
    /// an account replaces its row (a fresh `linked_at`, a live token).
    /// Refused with [`RefusalKind::GithubIdTaken`] when another account holds
    /// the same `(host, id)`, and with [`RefusalKind::NoSuchAccount`] when
    /// `account` does not exist. `host` is stored lowercase.
    pub async fn link_github(
        &self,
        account: &str,
        host: &str,
        github_id: u64,
        login: &str,
    ) -> Result<GithubLink> {
        self.link_github_inner(account, None, host, github_id, login)
            .await
    }

    /// [`AuthStore::link_github`] for a sign-in that began earlier: refused with
    /// [`RefusalKind::NoSuchAccount`] unless the name still holds the account
    /// made at `born` ([`AuthStore::account_born`]). Asked inside the write's
    /// transaction, so a link never lands on a later account under the name.
    pub async fn link_github_if_born(
        &self,
        account: &str,
        born: &str,
        host: &str,
        github_id: u64,
        login: &str,
    ) -> Result<GithubLink> {
        self.link_github_inner(account, Some(born), host, github_id, login)
            .await
    }

    async fn link_github_inner(
        &self,
        account: &str,
        born: Option<&str>,
        host: &str,
        github_id: u64,
        login: &str,
    ) -> Result<GithubLink> {
        let account = normalize_account_name(account)?;
        let host = normalize_github_host(host)?;
        let id = i64::try_from(github_id).context("a GitHub id out of range")?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("linking GitHub to user '{account}'"))?;
        let result = async {
            let found = self
                .query_first(
                    "SELECT created_at FROM users WHERE name = ?1",
                    vec![Value::Text(account.clone())],
                )
                .await?
                .and_then(|row| cell_text(&row, 0));
            let same = match (&found, born) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(found), Some(born)) => found == born,
            };
            if !same {
                return Err(refuse(
                    RefusalKind::NoSuchAccount,
                    format!("no such user: '{account}'"),
                ));
            }
            if self
                .query_first(
                    "SELECT 1 FROM github_link WHERE host = ?1 AND github_id = ?2 AND account <> ?3",
                    vec![
                        Value::Text(host.clone()),
                        Value::Integer(id),
                        Value::Text(account.clone()),
                    ],
                )
                .await?
                .is_some()
            {
                return Err(refuse(
                    RefusalKind::GithubIdTaken,
                    GITHUB_ID_TAKEN.to_string(),
                ));
            }
            let now = chrono::Utc::now();
            self.conn
                .execute(
                    "DELETE FROM github_link WHERE account = ?1",
                    vec![Value::Text(account.clone())],
                )
                .await
                .with_context(|| format!("replacing the GitHub link of user '{account}'"))?;
            self.conn
                .execute(
                    "INSERT INTO github_link (account, host, github_id, login, linked_at, token_live)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                    vec![
                        Value::Text(account.clone()),
                        Value::Text(host.clone()),
                        Value::Integer(id),
                        Value::Text(login.to_string()),
                        Value::Text(now.to_rfc3339()),
                    ],
                )
                .await
                .with_context(|| format!("linking GitHub to user '{account}'"))?;
            Ok(GithubLink {
                account: account.clone(),
                host: host.clone(),
                github_id,
                login: login.to_string(),
                linked_at: now,
                token_live: true,
            })
        }
        .await;
        self.finish(result).await
    }

    /// The GitHub link of one account, if it has one.
    pub async fn github_link_of(&self, account: &str) -> Result<Option<GithubLink>> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        Ok(self
            .query_first(
                &format!("SELECT {GITHUB_LINK_COLUMNS} FROM github_link WHERE account = ?1"),
                vec![Value::Text(account)],
            )
            .await?
            .map(|row| github_link_from_row(&row)))
    }

    /// The account linked to GitHub id `github_id` on `host`, if any.
    pub async fn account_for_github(&self, host: &str, github_id: u64) -> Result<Option<String>> {
        let host = normalize_github_host(host)?;
        let Ok(id) = i64::try_from(github_id) else {
            return Ok(None);
        };
        let _guard = self.guard.lock().await;
        Ok(self
            .query_first(
                "SELECT account FROM github_link WHERE host = ?1 AND github_id = ?2",
                vec![Value::Text(host), Value::Integer(id)],
            )
            .await?
            .and_then(|row| cell_text(&row, 0)))
    }

    /// Every GitHub link, by account.
    pub async fn github_links(&self) -> Result<Vec<GithubLink>> {
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                &format!("SELECT {GITHUB_LINK_COLUMNS} FROM github_link ORDER BY account"),
                (),
            )
            .await
            .context("reading the GitHub links")?;
        let mut links = Vec::new();
        while let Some(row) = rows.next().await.context("reading the GitHub links")? {
            links.push(github_link_from_row(&row));
        }
        Ok(links)
    }

    /// Refresh the cached login of an account's link after a rename on
    /// GitHub. False when the account has no link.
    pub async fn set_github_login(&self, account: &str, login: &str) -> Result<bool> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE github_link SET login = ?2 WHERE account = ?1",
                vec![Value::Text(account), Value::Text(login.to_string())],
            )
            .await
            .context("updating the GitHub login")?;
        Ok(changed > 0)
    }

    /// Record that the stored user token is gone; the link stays. False when
    /// the account has no link.
    pub async fn mark_github_token_gone(&self, account: &str) -> Result<bool> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE github_link SET token_live = 0 WHERE account = ?1",
                vec![Value::Text(account)],
            )
            .await
            .context("marking the GitHub token gone")?;
        Ok(changed > 0)
    }

    /// Remove an account's GitHub link. True when there was one.
    pub async fn unlink_github(&self, account: &str) -> Result<bool> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "DELETE FROM github_link WHERE account = ?1",
                vec![Value::Text(account)],
            )
            .await
            .context("removing the GitHub link")?;
        Ok(changed > 0)
    }

    /// Remember the GitHub webhook delivery `id`, received now. True when it
    /// is new, false when a delivery with this id was received in the last
    /// day (a replay, which does no work). See
    /// [`AuthStore::remember_delivery_at`].
    pub async fn remember_delivery(&self, id: &str) -> Result<bool> {
        self.remember_delivery_at(id, chrono::Utc::now().timestamp())
            .await
    }

    /// [`AuthStore::remember_delivery`] at `now` (unix seconds). Deliveries
    /// received more than [`DELIVERY_KEPT_SECS`] before `now` are pruned
    /// first, in the same transaction, so the table holds one day and an id
    /// older than that counts as new. The id is GitHub's
    /// `X-GitHub-Delivery`, stored as given.
    pub async fn remember_delivery_at(&self, id: &str, now: i64) -> Result<bool> {
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .context("remembering a GitHub delivery")?;
        let result = async {
            self.conn
                .execute(
                    "DELETE FROM github_delivery WHERE received_at < ?1",
                    vec![Value::Integer(now.saturating_sub(DELIVERY_KEPT_SECS))],
                )
                .await
                .context("pruning the GitHub deliveries")?;
            let inserted = self
                .conn
                .execute(
                    "INSERT INTO github_delivery (id, received_at) VALUES (?1, ?2)
                     ON CONFLICT (id) DO NOTHING",
                    vec![Value::Text(id.to_string()), Value::Integer(now)],
                )
                .await
                .context("remembering a GitHub delivery")?;
            Ok(inserted > 0)
        }
        .await;
        self.finish(result).await
    }

    /// Forget the GitHub webhook delivery `id`, so a redelivery of it is
    /// handled: called when handling it failed.
    pub async fn forget_delivery(&self, id: &str) -> Result<()> {
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM github_delivery WHERE id = ?1",
                vec![Value::Text(id.to_string())],
            )
            .await
            .context("forgetting a GitHub delivery")?;
        Ok(())
    }

    /// Create an account for an identity nobody has seen before, and link it,
    /// as one transaction.
    ///
    /// `desired_name` is a *hint*: it is folded, and a name already taken is
    /// uniquified with `-2`, `-3` and so on rather than joined. Joining would
    /// hand a stranger whose provider happens to call them `ada` whatever the
    /// local `ada` may do, which is exactly the takeover the `(issuer,
    /// subject)` rule exists to prevent.
    ///
    /// The account is created with no password hash: it signs in through its
    /// provider, and there is no local credential to guess.
    ///
    /// One `BEGIN IMMEDIATE` for the cap check, the name search, the insert
    /// and the link. Two first sign-ins racing therefore produce one account
    /// and one link, rather than an orphan account whose link lost.
    #[allow(clippy::too_many_arguments)]
    pub async fn provision_linked_user(
        &self,
        issuer: &str,
        subject: &str,
        desired_name: &str,
        display: Option<&str>,
        email: Option<&str>,
        role: Role,
        cap: usize,
    ) -> Result<User> {
        let issuer = identity_value(issuer, "issuer")?;
        let subject = identity_value(subject, "subject")?;
        let base = normalize_new_account_name(desired_name)?;
        let display = display
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let email = email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .context("provisioning an account for a single sign-on")?;
        let result = async {
            let count = match self
                .query_first("SELECT COUNT(*) FROM users", vec![])
                .await?
                .map(|row| row.get_value(0))
            {
                Some(Ok(Value::Integer(n))) => n as usize,
                _ => 0,
            };
            if count >= cap {
                return Err(refuse(
                    RefusalKind::CapReached,
                    format!(
                        "refusing to provision an account for this sign-in: the account cap is \
                         reached (auth.max_users = {cap}). Remove unused accounts or raise the cap"
                    ),
                ));
            }
            let name = self.free_account_name(&base).await?;
            self.conn
                .execute(
                    "INSERT INTO users
                         (name, display, email, role, pass_hash, disabled, created_at)
                     VALUES (?1, ?2, ?3, ?4, NULL, 0, ?5)",
                    vec![
                        Value::Text(name.clone()),
                        Value::Text(display.clone().unwrap_or_else(|| name.clone())),
                        match &email {
                            Some(value) => Value::Text(value.clone()),
                            None => Value::Null,
                        },
                        Value::Text(role.as_str().to_string()),
                        Value::Text(chrono::Utc::now().to_rfc3339()),
                    ],
                )
                .await
                .with_context(|| format!("provisioning user '{name}'"))?;
            self.insert_link(&issuer, &subject, &name, LINKED_BY_JIT)
                .await?;
            self.query_first(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
                vec![Value::Text(name.clone())],
            )
            .await?
            .map(|row| user_from_row(&row))
            .ok_or_else(|| anyhow!("user '{name}' vanished right after being provisioned"))
        }
        .await;
        self.finish(result).await
    }

    /// Refresh what an account shows: its display name and its address, and
    /// nothing else. Returns the account as it now stands, read back in the
    /// same call so a caller never has to guess what it wrote.
    ///
    /// A `None` leaves the stored value alone: an ID token that carries no
    /// `name` this time says nothing about the person's name, and must never
    /// be read as "they no longer have one". Role, disabled state and login
    /// name are untouched by construction - they are not presentation data,
    /// and a provider does not get to move them.
    pub async fn refresh_presentation(
        &self,
        name: &str,
        display: Option<&str>,
        email: Option<&str>,
    ) -> Result<User> {
        let name = normalize_account_name(name)?;
        let text = |value: Option<&str>| match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(value) => Value::Text(value.to_string()),
            None => Value::Null,
        };
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE users
                 SET display = COALESCE(?2, display), email = COALESCE(?3, email)
                 WHERE name = ?1",
                vec![Value::Text(name.clone()), text(display), text(email)],
            )
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        self.query_first(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
            vec![Value::Text(name.clone())],
        )
        .await?
        .map(|row| user_from_row(&row))
        .ok_or_else(|| anyhow!("no such user: '{name}'"))
    }

    /// Issue a session for an existing account, valid for `ttl_secs` from now.
    /// The returned token is the only unhashed copy; only its sha256 is
    /// written. A non-positive `ttl_secs` produces an already-expired session,
    /// which is how the expiry path is exercised without waiting.
    ///
    /// The existence check and the insert are one `BEGIN IMMEDIATE`
    /// transaction. Without it, a [`AuthStore::remove_user`] running in the
    /// CLI could delete the account between the two and leave this session
    /// stranded, to be inherited by the next account to claim the name.
    pub async fn create_session(&self, name: &str, ttl_secs: i64) -> Result<Session> {
        let name = normalize_account_name(name)?;
        let token = random_hex();
        let csrf = random_hex();
        let expires_at = chrono::Utc::now().timestamp().saturating_add(ttl_secs);
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("creating a session for user '{name}'"))?;
        let result = async {
            let exists = self
                .query_first(
                    "SELECT 1 FROM users WHERE name = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await?;
            if exists.is_none() {
                bail!("no such user: '{name}'");
            }
            self.conn
                .execute(
                    "INSERT INTO sessions (token_hash, user_name, csrf, expires_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    vec![
                        Value::Text(token_hash(&token)),
                        Value::Text(name.clone()),
                        Value::Text(csrf.clone()),
                        Value::Integer(expires_at),
                    ],
                )
                .await
                .with_context(|| format!("creating a session for user '{name}'"))?;
            Ok(())
        }
        .await;
        self.finish(result).await?;
        Ok(Session {
            token,
            csrf,
            expires_at,
        })
    }

    /// Reuse the account's newest live session, or issue one when it holds
    /// none.
    ///
    /// What `GET /auth/me` calls for a trusted-header identity, which arrives
    /// with an account and no session of its own. Minting unconditionally would
    /// add a row per probe - unbounded for a client that keeps no cookie - and
    /// would hand two tabs opening at once two different CSRF tokens, of which
    /// only the one whose `Set-Cookie` landed last would work.
    ///
    /// The check and the insert are one `BEGIN IMMEDIATE` transaction, which is
    /// the whole point: two concurrent probes serialize here, so the second sees
    /// the first's session rather than racing it to a duplicate.
    ///
    /// A reused session yields only its CSRF token. The session token itself is
    /// stored hashed and no unhashed copy is kept, so there is nothing to put in
    /// a cookie - which is why [`AuthStore::newest_session_csrf`] exists: the
    /// trusted-header path resolves the token by identity, not by cookie.
    pub async fn ensure_session(&self, name: &str, ttl_secs: i64) -> Result<SessionMint> {
        let name = normalize_account_name(name)?;
        let now = chrono::Utc::now().timestamp();
        let token = random_hex();
        let csrf = random_hex();
        let expires_at = now.saturating_add(ttl_secs);
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("ensuring a session for user '{name}'"))?;
        let mut reused = None;
        let result = async {
            let exists = self
                .query_first(
                    "SELECT 1 FROM users WHERE name = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await?;
            if exists.is_none() {
                bail!("no such user: '{name}'");
            }
            // Drop this account's expired rows while the transaction is open.
            // `session_user` is the only other pruner, and a probe that never
            // presents a cookie never reaches it: without this, an SSO identity
            // whose session lapsed would leave a dead row behind on every
            // expiry, forever. Scoped to one account rather than sweeping the
            // table, so the cost stays proportional to the caller.
            self.conn
                .execute(
                    "DELETE FROM sessions WHERE user_name = ?1 AND expires_at <= ?2",
                    vec![Value::Text(name.clone()), Value::Integer(now)],
                )
                .await
                .with_context(|| format!("pruning expired sessions for user '{name}'"))?;
            if let Some(live) = self.live_csrf(&name, now).await? {
                reused = Some(live);
                return Ok(());
            }
            self.conn
                .execute(
                    "INSERT INTO sessions (token_hash, user_name, csrf, expires_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    vec![
                        Value::Text(token_hash(&token)),
                        Value::Text(name.clone()),
                        Value::Text(csrf.clone()),
                        Value::Integer(expires_at),
                    ],
                )
                .await
                .with_context(|| format!("ensuring a session for user '{name}'"))?;
            Ok(())
        }
        .await;
        self.finish(result).await?;
        Ok(match reused {
            Some(csrf) => SessionMint::Reused { csrf },
            None => SessionMint::Created(Session {
                token,
                csrf,
                expires_at,
            }),
        })
    }

    /// The CSRF token of the newest live session `name` holds, or `None`.
    ///
    /// The trusted-header path resolves its token this way rather than through
    /// the session cookie: the proxy is what names the identity there, the
    /// cookie carries nothing the header does not already say, and a device
    /// whose cookie was never set or has gone stale would otherwise hold a token
    /// the server refuses to recognize. Not used by the cookie-session path,
    /// where the cookie is the identity and its own session's token is the one
    /// that must match.
    pub async fn newest_session_csrf(&self, name: &str) -> Result<Option<String>> {
        let name = normalize_account_name(name)?;
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        self.live_csrf(&name, now).await
    }

    /// Which account a live session belongs to, or `None` when the token names
    /// no live session.
    ///
    /// A pure read, unlike [`AuthStore::session_user`], which stamps
    /// `last_seen_at` for whoever it resolves. `GET /auth/me` asks this about a
    /// cookie it is deciding whether to retire, and the answer must not record
    /// the cookie's owner as having just been seen: they are not the one making
    /// the request.
    pub async fn session_owner(&self, token: &str) -> Result<Option<String>> {
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        Ok(self
            .query_first(
                "SELECT user_name FROM sessions
                 WHERE token_hash = ?1 AND expires_at > ?2",
                vec![Value::Text(token_hash(token)), Value::Integer(now)],
            )
            .await?
            .and_then(|row| cell_text(&row, 0)))
    }

    /// The account behind a live session, as [`AuthStore::session_user`]
    /// answers it, but as a pure read: nothing is pruned and `last_seen_at` is
    /// not stamped. `None` for a token that names no live session and for a
    /// disabled account.
    ///
    /// For a caller that re-checks a session it already holds rather than one
    /// serving a request: an open event stream asks this every few seconds for
    /// the life of a tab, and a write each time would both make every open tab
    /// read as a fresh sighting and queue every stream's check behind the
    /// store's writes.
    pub async fn session_user_quiet(&self, token: &str) -> Result<Option<User>> {
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        Ok(self
            .query_first(
                &format!(
                    "SELECT {USER_COLUMNS_JOINED}
                     FROM sessions s JOIN users u ON u.name = s.user_name
                     WHERE s.token_hash = ?1 AND s.expires_at > ?2"
                ),
                vec![Value::Text(token_hash(token)), Value::Integer(now)],
            )
            .await?
            .map(|row| user_from_row(&row))
            .filter(|user| !user.disabled))
    }

    /// How many session rows exist, live and expired alike. A diagnostic, and
    /// what pins the reuse invariant in tests: a probe that minted per call
    /// would show here as a growing count.
    pub async fn session_count(&self) -> Result<usize> {
        let _guard = self.guard.lock().await;
        Ok(
            match self
                .query_first("SELECT COUNT(*) FROM sessions", vec![])
                .await?
                .map(|row| row.get_value(0))
            {
                Some(Ok(Value::Integer(n))) => n as usize,
                _ => 0,
            },
        )
    }

    /// The newest unexpired session's CSRF token for an already-normalized
    /// `name`. Callers hold the guard; `ensure_session` also holds an open
    /// transaction, so this must not take either.
    ///
    /// Ordered by `expires_at` because the table records no creation time and
    /// the TTL is a constant, which makes the two orders the same. The tie-break
    /// on `token_hash` only keeps the answer stable when two sessions were
    /// issued in the same second.
    async fn live_csrf(&self, name: &str, now: i64) -> Result<Option<String>> {
        Ok(self
            .query_first(
                "SELECT csrf FROM sessions
                 WHERE user_name = ?1 AND expires_at > ?2
                 ORDER BY expires_at DESC, token_hash DESC
                 LIMIT 1",
                vec![Value::Text(name.to_string()), Value::Integer(now)],
            )
            .await?
            .and_then(|row| cell_text(&row, 0)))
    }

    /// Resolve a session token to its account and CSRF token. `None` for an
    /// unknown, expired or disabled-account session.
    ///
    /// Expired rows are pruned here rather than by a timer: every lookup is
    /// already a write-capable moment, sessions are only read on request, and
    /// a daemon that is never asked has no session rows worth reclaiming.
    ///
    /// The same sweep drops any session whose account no longer exists.
    /// [`AuthStore::remove_user`] is transactional so it cannot create one,
    /// but a file written by an older build could contain them, and an orphan
    /// is exactly what would be inherited by the next account to take the
    /// name. Clearing them on sight makes that unrecoverable rather than
    /// dormant.
    pub async fn session_user(&self, token: &str) -> Result<Option<(User, String)>> {
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM sessions
                 WHERE expires_at <= ?1
                    OR NOT EXISTS (SELECT 1 FROM users u WHERE u.name = sessions.user_name)",
                vec![Value::Integer(now)],
            )
            .await
            .context("pruning expired and orphaned sessions")?;
        let Some(row) = self
            .query_first(
                &format!(
                    "SELECT {USER_COLUMNS_JOINED}, s.csrf
                     FROM sessions s JOIN users u ON u.name = s.user_name
                     WHERE s.token_hash = ?1 AND s.expires_at > ?2"
                ),
                vec![Value::Text(token_hash(token)), Value::Integer(now)],
            )
            .await?
        else {
            return Ok(None);
        };
        let user = user_from_row(&row);
        if user.disabled {
            return Ok(None);
        }
        let csrf = cell_text(&row, 7).unwrap_or_default();
        self.conn
            .execute(
                "UPDATE users SET last_seen_at = ?2 WHERE name = ?1",
                vec![
                    Value::Text(user.name.clone()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .context("stamping last_seen_at")?;
        Ok(Some((user, csrf)))
    }

    /// Stamp `name` as seen just now.
    ///
    /// The sighting a forward-auth request is. That path resolves an account
    /// through its identity link rather than through a session or a
    /// provisioning call, so nothing else on it would move `last_seen_at` and
    /// an admin's user list would show every such account as never seen.
    /// [`AuthStore::ensure_user`] and [`AuthStore::session_user`] do the same
    /// write for the two paths they own; this is the third.
    ///
    /// `name` is an account name a row already carries, so it is stamped as
    /// given: a name that is nobody stamps nothing, which is not an error.
    pub async fn mark_seen(&self, name: &str) -> Result<()> {
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "UPDATE users SET last_seen_at = ?2 WHERE name = ?1",
                vec![
                    Value::Text(name.to_string()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .context("stamping last_seen_at")?;
        Ok(())
    }

    /// Revoke one session. Deleting an unknown token is not an error: logging
    /// out twice, or with a stale cookie, is a normal thing for a browser to
    /// do.
    pub async fn delete_session(&self, token: &str) -> Result<()> {
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM sessions WHERE token_hash = ?1",
                vec![Value::Text(token_hash(token))],
            )
            .await
            .context("deleting a session")?;
        Ok(())
    }

    /// Delete every session `name` holds. `name` must already be normalized.
    ///
    /// Called by the three operations that end an account's right to the
    /// sessions it was issued - a removal, a password change and a disabling -
    /// each of which calls it while holding the guard and inside its own
    /// transaction, so the revocation lands with the change or not at all.
    async fn delete_sessions_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM sessions WHERE user_name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing sessions for user '{name}'"))?;
        Ok(())
    }

    /// Delete every MCP token `name` holds. `name` must already be normalized.
    ///
    /// Called by [`AuthStore::remove_user`] and [`AuthStore::remove_user_force`]
    /// alongside [`AuthStore::delete_sessions_of`], inside the same transaction,
    /// so an account's tokens do not outlive it - the same resurrection risk
    /// [`AuthStore::remove_user`]'s doc comment describes for sessions applies
    /// here: a token row that outlives its account would be inherited by the
    /// next account to claim the freed name.
    ///
    /// Deliberately not called by [`AuthStore::set_disabled`]: disabling is
    /// reversible, and [`AuthStore::mcp_token_user`] already refuses a disabled
    /// account's tokens at read time, so re-enabling must hand every token back
    /// rather than force every integration to be re-issued.
    async fn delete_mcp_tokens_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM mcp_tokens WHERE user = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing mcp tokens for user '{name}'"))?;
        Ok(())
    }

    /// Issue a new MCP token for an existing account. The returned token is the
    /// only unhashed copy; only its sha256 is written, via the same
    /// `token_hash` helper a session token uses.
    ///
    /// Errors if the account does not exist, so a mistyped name is reported
    /// rather than silently minting an orphaned row.
    ///
    /// The existence check and the insert are one `BEGIN IMMEDIATE`
    /// transaction, exactly as [`AuthStore::create_session`] does it and for
    /// the same reason: without it, a `remove_user` running in the CLI could
    /// delete the account between the two and leave this token stranded, to be
    /// inherited by the next account to claim the name (`mcp_tokens` carries no
    /// foreign key, so nothing else would stop the insert from landing).
    pub async fn issue_mcp_token(&self, user: &str, label: &str) -> Result<IssuedMcpToken> {
        let user = normalize_account_name(user)?;
        let token = format!("{MCP_TOKEN_PREFIX}{}", random_hex());
        let hash = token_hash(&token);
        let created_at = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("issuing an mcp token for user '{user}'"))?;
        let result = async {
            let exists = self
                .query_first(
                    "SELECT 1 FROM users WHERE name = ?1",
                    vec![Value::Text(user.clone())],
                )
                .await?;
            if exists.is_none() {
                return Err(refuse(
                    RefusalKind::NoSuchAccount,
                    format!("no such user: '{user}'"),
                ));
            }
            self.insert_mcp_token_row(&user, &hash, label, &created_at)
                .await
        }
        .await;
        self.finish(result).await?;
        // Read after commit, still under `self.guard` and on this connection,
        // so nothing else on this connection can insert between the commit
        // above and this read - same reasoning `rotate_mcp_token` relies on.
        let id = self.conn.last_insert_rowid();
        Ok(IssuedMcpToken {
            id,
            token,
            label: label.to_string(),
        })
    }

    /// The `mcp_tokens` insert, shared by [`AuthStore::issue_mcp_token`] and
    /// [`AuthStore::add_token_only_user`] so both write a token the same way.
    /// The caller holds `self.guard` and an open transaction.
    async fn insert_mcp_token_row(
        &self,
        user: &str,
        hash: &str,
        label: &str,
        created_at: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO mcp_tokens (user, token_hash, label, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                vec![
                    Value::Text(user.to_string()),
                    Value::Text(hash.to_string()),
                    Value::Text(label.to_string()),
                    Value::Text(created_at.to_string()),
                ],
            )
            .await
            .with_context(|| format!("issuing an mcp token for user '{user}'"))?;
        Ok(())
    }

    /// Create an account with no password and issue one personal MCP token for
    /// it, in one transaction: an account for a deployed agent, which
    /// connects with the token and never signs in. The row has the same NULL
    /// `pass_hash` as one [`AuthStore::ensure_user`] provisions, so password
    /// sign-in never succeeds for it.
    ///
    /// The name is folded like [`AuthStore::add_user`] and a taken name is
    /// refused by the primary key, the same way. If the token cannot be
    /// written, the account is rolled back with it. Same lock and
    /// `BEGIN IMMEDIATE` pattern as [`AuthStore::issue_mcp_token`].
    pub async fn add_token_only_user(
        &self,
        name: &str,
        display: &str,
        email: Option<&str>,
        role: Role,
        label: &str,
    ) -> Result<IssuedMcpToken> {
        self.add_token_only_user_with_reflection(
            name,
            display,
            email,
            role,
            ReflectionLevel::Default,
            label,
        )
        .await
    }

    /// [`AuthStore::add_token_only_user`] with the account's Reflection level
    /// on every shared domain, written in the same transaction as the account
    /// and its token.
    pub async fn add_token_only_user_with_reflection(
        &self,
        name: &str,
        display: &str,
        email: Option<&str>,
        role: Role,
        reflection: ReflectionLevel,
        label: &str,
    ) -> Result<IssuedMcpToken> {
        let token = format!("{MCP_TOKEN_PREFIX}{}", random_hex());
        self.add_token_only_user_with(name, display, email, role, reflection, label, token)
            .await
    }

    /// [`AuthStore::add_token_only_user`] with the token already minted, the
    /// seam a test uses to force the token insert to fail.
    #[allow(clippy::too_many_arguments)]
    async fn add_token_only_user_with(
        &self,
        name: &str,
        display: &str,
        email: Option<&str>,
        role: Role,
        reflection: ReflectionLevel,
        label: &str,
        token: String,
    ) -> Result<IssuedMcpToken> {
        let name = normalize_new_account_name(name)?;
        let hash = token_hash(&token);
        let created_at = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("adding user '{name}'"))?;
        let result = async {
            self.conn
                .execute(
                    "INSERT INTO users
                         (name, display, email, role, reflection, pass_hash, disabled, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, ?6)",
                    vec![
                        Value::Text(name.clone()),
                        Value::Text(display.to_string()),
                        match email {
                            Some(e) => Value::Text(e.to_string()),
                            None => Value::Null,
                        },
                        Value::Text(role.as_str().to_string()),
                        reflection.to_db(),
                        Value::Text(created_at.clone()),
                    ],
                )
                .await
                .with_context(|| format!("adding user '{name}'"))?;
            self.insert_mcp_token_row(&name, &hash, label, &created_at)
                .await
        }
        .await;
        self.finish(result).await?;
        let id = self.conn.last_insert_rowid();
        Ok(IssuedMcpToken {
            id,
            token,
            label: label.to_string(),
        })
    }

    /// Resolve an MCP token to its account. `None` for an unknown, revoked (the
    /// row is gone, whether by an explicit revoke or by the account's removal),
    /// or disabled-account token - the three are deliberately indistinguishable,
    /// same as [`AuthStore::verify_password`]. Stamps `last_used` on a hit, so
    /// [`AuthStore::list_mcp_tokens`] can show when a token was last presented.
    ///
    /// Also prunes any orphaned `mcp_tokens` row - one whose account no longer
    /// exists - on every call, the same defense in depth
    /// [`AuthStore::session_user`] applies to sessions. `mcp_tokens` carries no
    /// foreign key, and [`AuthStore::issue_mcp_token`]'s transaction is what is
    /// supposed to stop a stranded row from ever being written; this is the
    /// belt to that transaction's suspenders, for a row written before this
    /// existed or reached by any other path.
    pub async fn mcp_token_user(&self, token: &str) -> Result<Option<User>> {
        let hash = token_hash(token);
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM mcp_tokens
                 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.name = mcp_tokens.user)",
                (),
            )
            .await
            .context("pruning orphaned mcp tokens")?;
        let Some(row) = self
            .query_first(
                &format!(
                    "SELECT {USER_COLUMNS_JOINED}, m.id
                     FROM mcp_tokens m JOIN users u ON u.name = m.user
                     WHERE m.token_hash = ?1"
                ),
                vec![Value::Text(hash)],
            )
            .await?
        else {
            return Ok(None);
        };
        let user = user_from_row(&row);
        if user.disabled {
            return Ok(None);
        }
        if let Ok(Value::Integer(id)) = row.get_value(7) {
            self.conn
                .execute(
                    "UPDATE mcp_tokens SET last_used = ?2 WHERE id = ?1",
                    vec![
                        Value::Integer(id),
                        Value::Text(chrono::Utc::now().to_rfc3339()),
                    ],
                )
                .await
                .context("stamping an mcp token's last_used")?;
        }
        Ok(Some(user))
    }

    /// Every MCP token `user` holds, newest first, never carrying the token
    /// itself. `user` is folded by [`normalize_account_name`] like every other lookup
    /// keyed on a login name.
    pub async fn list_mcp_tokens(&self, user: &str) -> Result<Vec<McpTokenInfo>> {
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT id, label, created_at, last_used FROM mcp_tokens
                 WHERE user = ?1 ORDER BY created_at DESC, id DESC",
                vec![Value::Text(user.clone())],
            )
            .await
            .with_context(|| format!("listing mcp tokens for user '{user}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing mcp tokens for user '{user}'"))?
        {
            let Ok(Value::Integer(id)) = row.get_value(0) else {
                continue;
            };
            out.push(McpTokenInfo {
                id,
                label: cell_text(&row, 1).unwrap_or_default(),
                created_at: cell_text(&row, 2).unwrap_or_default(),
                last_used: cell_text(&row, 3),
            });
        }
        Ok(out)
    }

    /// Revoke one of `user`'s MCP tokens by id. Returns whether a row was
    /// deleted - `false` covers both an unknown id and one owned by a different
    /// account, deliberately indistinguishable so a caller cannot probe another
    /// account's token ids.
    pub async fn revoke_mcp_token(&self, user: &str, id: i64) -> Result<bool> {
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "DELETE FROM mcp_tokens WHERE id = ?1 AND user = ?2",
                vec![Value::Integer(id), Value::Text(user.clone())],
            )
            .await
            .with_context(|| format!("revoking an mcp token for user '{user}'"))?;
        Ok(changed > 0)
    }

    /// Replace one of `user`'s MCP tokens with a freshly issued one carrying
    /// the same label, in one transaction: the old row is gone and the new one
    /// exists, or neither change happened. Errors if `id` does not name a token
    /// owned by `user`.
    pub async fn rotate_mcp_token(&self, user: &str, id: i64) -> Result<IssuedMcpToken> {
        let user = normalize_account_name(user)?;
        let token = format!("{MCP_TOKEN_PREFIX}{}", random_hex());
        let hash = token_hash(&token);
        let created_at = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("rotating an mcp token for user '{user}'"))?;
        // Captured by the closure below and read after `finish` commits, the
        // same `ensure_session` shape this file already uses for a
        // transaction whose caller needs more out of it than `Result<()>`.
        let mut label = String::new();
        let result = async {
            let row = self
                .query_first(
                    "SELECT label FROM mcp_tokens WHERE id = ?1 AND user = ?2",
                    vec![Value::Integer(id), Value::Text(user.clone())],
                )
                .await?;
            let Some(row) = row else {
                return Err(refuse(
                    RefusalKind::NoSuchToken,
                    format!("no such mcp token '{id}' for user '{user}'"),
                ));
            };
            label = cell_text(&row, 0).unwrap_or_default();
            self.conn
                .execute(
                    "DELETE FROM mcp_tokens WHERE id = ?1",
                    vec![Value::Integer(id)],
                )
                .await
                .with_context(|| format!("rotating an mcp token for user '{user}'"))?;
            self.conn
                .execute(
                    "INSERT INTO mcp_tokens (user, token_hash, label, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    vec![
                        Value::Text(user.clone()),
                        Value::Text(hash.clone()),
                        Value::Text(label.clone()),
                        Value::Text(created_at.clone()),
                    ],
                )
                .await
                .with_context(|| format!("rotating an mcp token for user '{user}'"))?;
            Ok(())
        }
        .await;
        self.finish(result).await?;
        // Read after commit, still under `self.guard` and on this connection:
        // see `issue_mcp_token`'s matching comment.
        let id = self.conn.last_insert_rowid();
        Ok(IssuedMcpToken { id, token, label })
    }

    /// Mint a share-link on one of `owner`'s drafts. The returned token is the
    /// only unhashed copy; only its sha256 is written, through the same
    /// `token_hash` helper every other credential here goes through.
    ///
    /// The store keeps no opinion about whether `owner` actually holds a draft
    /// at `path`: that is a question about the overlay, which lives in the
    /// index and not in this database, and the route that mints is where it is
    /// asked (and answered 404 when the answer is no). What this layer owns is
    /// the link itself - that it is unguessable, that it names one draft, and
    /// that it can be taken back.
    ///
    /// `expires_at` is RFC 3339 or nothing at all. Nothing is the normal case
    /// and means the link lasts exactly as long as the draft does, which is
    /// the whole lifetime a draft has: [`AuthStore::end_overlay_grants`] ends
    /// it when the draft is folded or discarded.
    pub async fn mint_overlay_grant(
        &self,
        domain: &str,
        path: &str,
        owner: &str,
        expires_at: Option<String>,
    ) -> Result<MintedGrant> {
        let owner = normalize_account_name(owner)?;
        let token = format!("{DRAFT_LINK_PREFIX}{}", random_hex());
        let hash = token_hash(&token);
        let created_at = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "INSERT INTO overlay_grant
                     (domain, path, owner, grantee, token_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                vec![
                    Value::Text(domain.to_string()),
                    Value::Text(path.to_string()),
                    Value::Text(owner.clone()),
                    Value::Text(hash),
                    Value::Text(created_at),
                    match expires_at {
                        Some(at) => Value::Text(at),
                        None => Value::Null,
                    },
                ],
            )
            .await
            .with_context(|| format!("minting a draft share-link for '{owner}'"))?;
        // Read after the insert, still under `self.guard` and on this
        // connection: see `issue_mcp_token`'s matching comment.
        Ok(MintedGrant {
            id: self.conn.last_insert_rowid(),
            token,
        })
    }

    /// Present a link. `Some` when it opens something for `account`, `None`
    /// for every way it does not.
    ///
    /// **The first account to present a live link is its grantee for good.**
    /// That is the whole of the sharing rule: the author mints a link for a
    /// draft rather than for a name, so they never have to know the login of
    /// whoever they are handing it to, and the moment somebody opens it the
    /// link stops being a bearer credential and becomes a grant to one
    /// account. A second account is answered exactly as an invented token is -
    /// a link that admitted a second reader would be a draft shared with
    /// whoever the first reader forwarded it to.
    ///
    /// The read and the binding are one `BEGIN IMMEDIATE` transaction, for the
    /// reason [`AuthStore::issue_mcp_token`]'s pair is: two accounts opening
    /// one fresh link at the same instant must not both be told they bound it.
    ///
    /// An unknown, revoked or expired link is one answer, deliberately, the
    /// way [`AuthStore::mcp_token_user`] answers one `None` to four misses.
    pub async fn redeem_overlay_grant(
        &self,
        token: &str,
        account: &str,
    ) -> Result<Option<OverlayGrant>> {
        let account = normalize_account_name(account)?;
        let hash = token_hash(token);
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .context("redeeming a draft share-link")?;
        let mut bound: Option<OverlayGrant> = None;
        let result = async {
            let row = self
                .query_first(
                    "SELECT id, domain, path, owner, grantee, created_at, expires_at, revoked_at
                     FROM overlay_grant WHERE token_hash = ?1",
                    vec![Value::Text(hash)],
                )
                .await?;
            let Some(grant) = row.as_ref().map(grant_from_row) else {
                return Ok(());
            };
            if !grant.is_live() {
                return Ok(());
            }
            match grant.grantee.as_deref() {
                // Already this account's: presenting it again is how a second
                // tab, or a reload, gets back what it already holds.
                Some(held) if held == account => bound = Some(grant),
                Some(_) => {}
                None => {
                    self.conn
                        .execute(
                            "UPDATE overlay_grant SET grantee = ?1
                             WHERE id = ?2 AND grantee IS NULL",
                            vec![Value::Text(account.clone()), Value::Integer(grant.id)],
                        )
                        .await
                        .context("binding a draft share-link")?;
                    bound = Some(OverlayGrant {
                        grantee: Some(account.clone()),
                        ..grant
                    });
                }
            }
            Ok(())
        }
        .await;
        self.finish(result).await?;
        Ok(bound)
    }

    /// Every link standing on one of `owner`'s drafts, newest first. Revoked
    /// and expired rows are left out: the author's list is what still opens
    /// the draft, which is the only question the list is asked.
    pub async fn overlay_grants_of(
        &self,
        owner: &str,
        domain: &str,
        path: &str,
    ) -> Result<Vec<OverlayGrant>> {
        let owner = normalize_account_name(owner)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT id, domain, path, owner, grantee, created_at, expires_at, revoked_at
                 FROM overlay_grant
                 WHERE owner = ?1 AND domain = ?2 AND path = ?3 AND revoked_at IS NULL
                 ORDER BY id DESC",
                vec![
                    Value::Text(owner.clone()),
                    Value::Text(domain.to_string()),
                    Value::Text(path.to_string()),
                ],
            )
            .await
            .with_context(|| format!("listing draft share-links for '{owner}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing draft share-links for '{owner}'"))?
        {
            let grant = grant_from_row(&row);
            if grant.is_live() {
                out.push(grant);
            }
        }
        Ok(out)
    }

    /// Take one link back, and answer the row it was. `None` when `id` names no
    /// link this caller may end, which is what somebody else's link and an
    /// invented id both answer: a revoke is never a probe for which links
    /// exist.
    ///
    /// **Whose link it is, or an admin's instance.** The author may always end
    /// what she minted, whatever her role has become since - a live credential
    /// must never be harder to take back than it was to hand out, or a
    /// departed author leaves a grant nobody alive can close. `as_admin`
    /// widens the same query to any link on the instance, which is the third
    /// party that case needs. Both statements below carry the same widening:
    /// a row the read matched and the update did not would be a revoke that
    /// answered a draft it never took a link off.
    ///
    /// **The row rather than a yes**, because taking a link back is not only a
    /// fact about the row: a session may be working inside the draft it opened
    /// at this moment, and the caller has to be told which draft that is to put
    /// them back outside it. Read here rather than asked for separately, since
    /// the row is this store's own word about the draft - already normalized,
    /// and already the one the update matched.
    ///
    /// The row is kept and stamped rather than deleted, so a link presented
    /// after the fact is a link that was taken back rather than a token that
    /// was never minted, and so the author's own audit of who they shared with
    /// survives the revoke. The record answered carries the stamp, so what the
    /// caller reads is the link as it now stands.
    pub async fn revoke_overlay_grant(
        &self,
        owner: &str,
        id: i64,
        as_admin: bool,
    ) -> Result<Option<OverlayGrant>> {
        let owner = normalize_account_name(owner)?;
        let revoked_at = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        // One predicate and one parameter list, built once and used by both
        // statements: an admin ends any link on the instance, everybody else
        // exactly their own. Read and then stamp, both under the one guard and
        // both carrying this same widening - a row the read matched and the
        // update did not would be a revoke that answered a draft it never took
        // a link off.
        let mine = if as_admin { "" } else { " AND owner = ?2" };
        let mut params = vec![Value::Integer(id)];
        if !as_admin {
            params.push(Value::Text(owner.clone()));
        }
        let mut rows = self
            .conn
            .query(
                &format!(
                    "SELECT id, domain, path, owner, grantee, created_at, expires_at, revoked_at
                 FROM overlay_grant
                 WHERE id = ?1{mine} AND revoked_at IS NULL"
                ),
                params,
            )
            .await
            .with_context(|| format!("revoking a draft share-link for '{owner}'"))?;
        let found = rows
            .next()
            .await
            .with_context(|| format!("revoking a draft share-link for '{owner}'"))?
            .map(|row| grant_from_row(&row));
        let Some(mut grant) = found else {
            return Ok(None);
        };
        let mine = if as_admin { "" } else { " AND owner = ?3" };
        let mut params = vec![Value::Text(revoked_at.clone()), Value::Integer(id)];
        if !as_admin {
            params.push(Value::Text(owner.clone()));
        }
        self.conn
            .execute(
                &format!(
                    "UPDATE overlay_grant SET revoked_at = ?1
                 WHERE id = ?2{mine} AND revoked_at IS NULL"
                ),
                params,
            )
            .await
            .with_context(|| format!("revoking a draft share-link for '{owner}'"))?;
        grant.revoked_at = Some(revoked_at);
        Ok(Some(grant))
    }

    /// Whose draft `account` may see at that path, or `None` for the ordinary
    /// case where the answer is nobody's.
    ///
    /// The one question the rest of the service asks this table, and it is
    /// deliberately narrow: it names a path, so it can widen what one account
    /// sees at exactly one address and can never be folded into a listing or a
    /// search. Links are the only cross-overlay visibility there is, and this
    /// is the whole of the seam they reach through.
    pub async fn overlay_grant_for(
        &self,
        account: &str,
        domain: &str,
        path: &str,
    ) -> Result<Option<String>> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT id, domain, path, owner, grantee, created_at, expires_at, revoked_at
                 FROM overlay_grant
                 WHERE grantee = ?1 AND domain = ?2 AND path = ?3 AND revoked_at IS NULL
                 ORDER BY id DESC",
                vec![
                    Value::Text(account.clone()),
                    Value::Text(domain.to_string()),
                    Value::Text(path.to_string()),
                ],
            )
            .await
            .with_context(|| format!("reading a draft share-link for '{account}'"))?;
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("reading a draft share-link for '{account}'"))?
        {
            let grant = grant_from_row(&row);
            if grant.is_live() {
                return Ok(Some(grant.owner));
            }
        }
        Ok(None)
    }

    /// Which domain a link is for, without binding it to anybody. `None` for
    /// every way the link opens nothing.
    ///
    /// The one read that exists so the bind can happen LAST. Redeeming binds a
    /// link to its first presenter for good, so an account that turns out not
    /// to be allowed to read the domain the draft is in would otherwise burn
    /// the link permanently - and be told nothing, since the refusal is the
    /// same 404 an invented token gets - leaving the person it was meant for
    /// unable to open it at all. Asking first costs one row read and makes the
    /// screen come before the irreversible half.
    pub async fn overlay_grant_domain(&self, token: &str) -> Result<Option<String>> {
        let hash = token_hash(token);
        let _guard = self.guard.lock().await;
        let row = self
            .query_first(
                "SELECT id, domain, path, owner, grantee, created_at, expires_at, revoked_at
                 FROM overlay_grant WHERE token_hash = ?1",
                vec![Value::Text(hash)],
            )
            .await?;
        Ok(row
            .as_ref()
            .map(grant_from_row)
            .filter(OverlayGrant::is_live)
            .map(|grant| grant.domain))
    }

    /// Every live link `account` holds in one domain.
    ///
    /// The plural of [`AuthStore::overlay_grant_for`], and it exists for one
    /// caller: a write addressed by NAME at a draft this account was granted.
    /// A name cannot be resolved without a view, and the grantee's own view
    /// does not carry the owner's draft, so the write path has to ask "is any
    /// draft you hold a link to the one you just named" - which is this list,
    /// and nothing wider. It carries no content and never leaves the write
    /// path.
    pub async fn overlay_grants_held(
        &self,
        account: &str,
        domain: &str,
    ) -> Result<Vec<OverlayGrant>> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT id, domain, path, owner, grantee, created_at, expires_at, revoked_at
                 FROM overlay_grant
                 WHERE grantee = ?1 AND domain = ?2 AND revoked_at IS NULL
                 ORDER BY id DESC",
                vec![
                    Value::Text(account.clone()),
                    Value::Text(domain.to_string()),
                ],
            )
            .await
            .with_context(|| format!("listing the draft share-links of '{account}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing the draft share-links of '{account}'"))?
        {
            let grant = grant_from_row(&row);
            if grant.is_live() {
                out.push(grant);
            }
        }
        Ok(out)
    }

    /// End every link standing on ONE draft, because that draft has ended.
    /// Answers how many were standing.
    ///
    /// A grant lasts exactly as long as the thing it grants, and "ends" has to
    /// mean ended rather than dormant: a row left live merely looks dead while
    /// nothing stands at the path, and springs back onto whatever its author
    /// drafts there next - a different text, written after they took the first
    /// one back, handed to somebody neither of them would have told.
    pub async fn end_overlay_grants(&self, domain: &str, owner: &str, path: &str) -> Result<u64> {
        let owner = normalize_account_name(owner)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE overlay_grant SET revoked_at = ?1
                 WHERE domain = ?2 AND owner = ?3 AND path = ?4 AND revoked_at IS NULL",
                vec![
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                    Value::Text(domain.to_string()),
                    Value::Text(owner.clone()),
                    Value::Text(path.to_string()),
                ],
            )
            .await
            .with_context(|| format!("ending the draft share-links of '{owner}'"))?;
        Ok(changed)
    }

    /// End every link `owner` gave, in every domain: a name is about to be used
    /// again, and a link the old person gave must not open what the new person
    /// drafts at that path.
    pub async fn end_overlay_grants_of_owner(&self, owner: &str) -> Result<u64> {
        let owner = normalize_account_name(owner)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE overlay_grant SET revoked_at = ?1
                 WHERE owner = ?2 AND revoked_at IS NULL",
                vec![
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                    Value::Text(owner.clone()),
                ],
            )
            .await
            .with_context(|| format!("ending the draft share-links of '{owner}'"))?;
        Ok(changed)
    }

    /// End every link on every draft in one domain, because the domain has
    /// stopped reviewing changes and no draft in it survived that.
    ///
    /// Called when a domain leaves review mode and when one is unregistered,
    /// the two moments that end every draft in it at once. The per-draft half
    /// is [`AuthStore::end_overlay_grants`] above. Answers how many were
    /// standing.
    pub async fn end_domain_overlay_grants(&self, domain: &str) -> Result<u64> {
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE overlay_grant SET revoked_at = ?1
                 WHERE domain = ?2 AND revoked_at IS NULL",
                vec![
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                    Value::Text(domain.to_string()),
                ],
            )
            .await
            .with_context(|| format!("ending the draft share-links of '{domain}'"))?;
        Ok(changed)
    }

    /// Register a public OAuth client and hand the record back, `client_id`
    /// and all. RFC 7591 dynamic registration is what calls this: a client
    /// nobody configured says what it is called and where it may be redirected
    /// to, and gets an id and no secret.
    ///
    /// What a redirect uri is allowed to be, how long a name may be and which
    /// authentication methods are refused belong to the endpoint that speaks
    /// RFC 7591's error codes. The one thing insisted on here is the one a
    /// stored row would be useless without: somewhere to redirect. A
    /// registration with no uri could never finish a flow, and
    /// [`AuthStore::list_oauth_grants`] reads the first one to tell a person
    /// which client a grant belongs to.
    pub async fn register_oauth_client(
        &self,
        name: &str,
        client_uri: Option<&str>,
        redirect_uris: &[String],
    ) -> Result<OauthClient> {
        if redirect_uris.is_empty() {
            bail!("an oauth client registration needs at least one redirect uri");
        }
        let client = OauthClient {
            client_id: format!("{OAUTH_CLIENT_PREFIX}{}", random_id_hex()),
            client_name: name.to_string(),
            client_uri: client_uri.map(str::to_string),
            redirect_uris: redirect_uris.to_vec(),
            created_at: chrono::Utc::now().to_rfc3339(),
            last_used: None,
        };
        let encoded = serde_json::to_string(&client.redirect_uris)
            .context("encoding an oauth client's redirect uris")?;
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "INSERT INTO oauth_clients
                     (client_id, client_name, client_uri, redirect_uris, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                vec![
                    Value::Text(client.client_id.clone()),
                    Value::Text(client.client_name.clone()),
                    match &client.client_uri {
                        Some(uri) => Value::Text(uri.clone()),
                        None => Value::Null,
                    },
                    Value::Text(encoded),
                    Value::Text(client.created_at.clone()),
                ],
            )
            .await
            .context("registering an oauth client")?;
        Ok(client)
    }

    /// One registration by id, or `None` when there is none - which is what an
    /// unknown `client_id` at the authorize or token endpoint looks like.
    pub async fn oauth_client(&self, client_id: &str) -> Result<Option<OauthClient>> {
        let _guard = self.guard.lock().await;
        self.oauth_client_row(client_id).await
    }

    /// [`AuthStore::oauth_client`] without the lock, for the methods that
    /// already hold it.
    async fn oauth_client_row(&self, client_id: &str) -> Result<Option<OauthClient>> {
        let Some(row) = self
            .query_first(
                "SELECT client_id, client_name, client_uri, redirect_uris, created_at, last_used
                 FROM oauth_clients WHERE client_id = ?1",
                vec![Value::Text(client_id.to_string())],
            )
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(OauthClient {
            client_id: cell_text(&row, 0).unwrap_or_default(),
            client_name: cell_text(&row, 1).unwrap_or_default(),
            client_uri: cell_text(&row, 2),
            redirect_uris: decode_redirect_uris(cell_text(&row, 3).as_deref()),
            created_at: cell_text(&row, 4).unwrap_or_default(),
            last_used: cell_text(&row, 5),
        }))
    }

    /// Record that a person AUTHORIZED this registration, which is the only
    /// thing `last_used` means and the whole input to the long clock in
    /// [`AuthStore::prune_oauth_clients`].
    ///
    /// **Call this when consent is allowed, and when a grant is issued or
    /// refreshed for the client. Never on an authorization request.** The
    /// authorize leg runs before anybody has signed in, and a caller that
    /// registered a client is authorizing against its own row, so every check
    /// there passes by construction: stamping on that request would let one
    /// extra unauthenticated call per row move it onto the thirty-day branch,
    /// which is exactly the fill-and-hold the one-hour clock exists to prevent
    /// (see [`OAUTH_CLIENT_UNAUTHORIZED_SECS`]). Reading a registration is not
    /// using it. The call sites are the consent endpoint's allow decision and
    /// the token endpoint; nothing else in this file writes the column, and
    /// `only_an_allowed_authorization_stamps_a_registration_as_used` is that
    /// contract in executable form.
    ///
    /// A `client_id` that names no registration is a no-op rather than an
    /// error: the caller has already refused the request it was stamping for,
    /// and a second failure mode here would tell it nothing new.
    pub async fn touch_oauth_client(&self, client_id: &str) -> Result<()> {
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "UPDATE oauth_clients SET last_used = ?2 WHERE client_id = ?1",
                vec![
                    Value::Text(client_id.to_string()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .context("stamping an oauth client registration")?;
        Ok(())
    }

    /// Delete every grant whose refresh window has closed, and report how
    /// many went. Runs at every registration and at daemon start, immediately
    /// **before** [`AuthStore::prune_oauth_clients`].
    ///
    /// A grant past `refresh_expires_at` can never work again: the rotation
    /// lookup excludes it, its access token died an hour into those thirty
    /// days, and a token that was never rotated cannot match the replay
    /// lookup. Nothing else would ever delete it, so without this the table
    /// grows with every re-authorization, the account's grant card lists
    /// connections that cannot work, and - the reason the order matters - each
    /// dead row holds its registration against the client prune's
    /// `NOT EXISTS`, which is exactly the abandoned registration that prune
    /// exists to collect.
    ///
    /// Expiry is the statement's comparison, like every other one here.
    pub async fn prune_oauth_grants(&self) -> Result<usize> {
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM oauth_grant_token WHERE grant_id IN
                     (SELECT id FROM oauth_grants WHERE refresh_expires_at <= ?1)",
                vec![Value::Integer(now)],
            )
            .await
            .context("pruning the tokens of expired oauth grants")?;
        let removed = self
            .conn
            .execute(
                "DELETE FROM oauth_grants WHERE refresh_expires_at <= ?1",
                vec![Value::Integer(now)],
            )
            .await
            .context("pruning expired oauth grants")?;
        // The ended-grant records age out on the same schedule: nothing
        // lists one past its seven days, so nothing needs to keep it.
        self.conn
            .execute(
                "DELETE FROM oauth_grant_ended WHERE ended_at <= ?1",
                vec![Value::Integer(now - OAUTH_ENDED_KEPT_SECS)],
            )
            .await
            .context("pruning ended oauth grant records")?;
        Ok(removed as usize)
    }

    /// Delete every registration that holds no grant and has aged out, and
    /// report how many went. Runs at every registration and at daemon start,
    /// immediately **after** [`AuthStore::prune_oauth_grants`] - a registration
    /// whose only grant is dead is only collectable once that grant is gone, so
    /// the two prunes in that order collect an abandoned client in one pass.
    ///
    /// The `NOT EXISTS` is the load-bearing half: a registration somebody is
    /// connected through is kept however old it is, because deleting it would
    /// leave live grants pointing at nothing.
    ///
    /// **Two ages, decided by whether a person ever authorized the client.** A
    /// row with `last_used IS NULL` - nobody has allowed it anything, whatever
    /// requests it has made - is collected [`OAUTH_CLIENT_UNAUTHORIZED_SECS`]
    /// after it was made; one that has been authorized at least once is
    /// collected [`OAUTH_CLIENT_UNUSED_SECS`] after that last authorization.
    /// See the two constants for why they differ by that much, and
    /// [`AuthStore::touch_oauth_client`] for why the stamp follows a person's
    /// decision rather than the arrival of a request: the short clock is what
    /// keeps a table filled by an anonymous caller from staying full for a
    /// month, and a clock keyed on requests would hand that month back for one
    /// extra call per row.
    ///
    /// Every date compared here is RFC 3339 UTC written by this file, so byte
    /// order is time order - the same property
    /// [`AuthStore::list_mcp_tokens`] already orders on.
    pub async fn prune_oauth_clients(&self) -> Result<usize> {
        let now = chrono::Utc::now();
        let idle_cutoff = (now - chrono::Duration::seconds(OAUTH_CLIENT_UNUSED_SECS)).to_rfc3339();
        let unauthorized_cutoff =
            (now - chrono::Duration::seconds(OAUTH_CLIENT_UNAUTHORIZED_SECS)).to_rfc3339();
        let _guard = self.guard.lock().await;
        let removed = self
            .conn
            .execute(
                "DELETE FROM oauth_clients
                 WHERE NOT EXISTS (
                           SELECT 1 FROM oauth_grants g
                           WHERE g.client_id = oauth_clients.client_id
                       )
                   AND CASE
                           WHEN last_used IS NULL THEN created_at < ?2
                           ELSE last_used < ?1
                       END",
                vec![Value::Text(idle_cutoff), Value::Text(unauthorized_cutoff)],
            )
            .await
            .context("pruning unused oauth client registrations")?;
        Ok(removed as usize)
    }

    /// How many registrations are stored, for the cap the registration
    /// endpoint enforces. That one caller is the whole of it; nothing reports
    /// this number on a settings surface.
    pub async fn count_oauth_clients(&self) -> Result<usize> {
        let _guard = self.guard.lock().await;
        // A count this method cannot read fails the call rather than reading
        // as an empty table: the one caller is the ceiling on registration,
        // the only unauthenticated write here, and a zero there would admit
        // the very row the ceiling exists to refuse.
        match self
            .query_first("SELECT COUNT(*) FROM oauth_clients", vec![])
            .await?
            .map(|row| row.get_value(0))
        {
            Some(Ok(Value::Integer(n))) => Ok(n as usize),
            _ => bail!("the oauth client registrations could not be counted"),
        }
    }

    /// Issue a grant: one row naming the account, the client and the
    /// resource, plus its first token pair in `oauth_grant_token`: an access
    /// token good for an hour and a refresh token good for thirty days, both
    /// hashed. The returned tokens are the only unhashed copies.
    ///
    /// The account check and the registration check share one `BEGIN
    /// IMMEDIATE` with the inserts, for the reason
    /// [`AuthStore::issue_mcp_token`] documents: neither table carries a
    /// foreign key, so without the transaction a concurrent `remove_user` or
    /// prune could leave this grant pointing at nothing.
    ///
    /// `resource` is stored as [`normalize_resource`] spells it, so the
    /// audience comparison at read time is an equality test on one spelling
    /// rather than a family of them.
    pub async fn issue_oauth_grant(
        &self,
        user: &str,
        client_id: &str,
        resource: &str,
    ) -> Result<IssuedOauthGrant> {
        let user = normalize_account_name(user)?;
        let resource = normalize_resource(resource);
        // The empty string is not an audience. Refused here and refused again
        // at the read, so the two halves of the check can never agree on
        // nothing - not even if an origin derivation upstream produced one.
        if resource.is_empty() {
            bail!("an oauth grant needs a resource to be issued for");
        }
        let pair = TokenPair::new();
        let now = chrono::Utc::now();
        let issued_at = now.timestamp();
        let created_at = now.to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("issuing an oauth grant for user '{user}'"))?;
        let mut id = 0;
        let result = async {
            self.require_live_user(&user).await?;
            if self
                .query_first(
                    "SELECT 1 FROM oauth_clients WHERE client_id = ?1",
                    vec![Value::Text(client_id.to_string())],
                )
                .await?
                .is_none()
            {
                bail!("no such oauth client: '{client_id}'");
            }
            // The token columns of `oauth_grants` get inert placeholders: see
            // `OAUTH_PLACEHOLDER_PREFIX`. The real pair goes to
            // `oauth_grant_token` just below.
            self.conn
                .execute(
                    "INSERT INTO oauth_grants
                         (user, client_id, resource, access_hash, access_expires_at,
                          refresh_hash, refresh_expires_at, created_at)
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
                    vec![
                        Value::Text(user.clone()),
                        Value::Text(client_id.to_string()),
                        Value::Text(resource.clone()),
                        Value::Text(format!("{OAUTH_PLACEHOLDER_PREFIX}access:{}", random_hex())),
                        Value::Text(format!(
                            "{OAUTH_PLACEHOLDER_PREFIX}refresh:{}",
                            random_hex()
                        )),
                        Value::Integer(issued_at + OAUTH_REFRESH_TTL_SECS),
                        Value::Text(created_at),
                    ],
                )
                .await
                .with_context(|| format!("issuing an oauth grant for user '{user}'"))?;
            // Read inside the transaction, still under `self.guard` and on
            // this connection: see `issue_mcp_token`'s matching comment.
            id = self.conn.last_insert_rowid();
            self.mint_oauth_token(id, &pair, None, issued_at).await
        }
        .await;
        self.finish(result).await?;
        Ok(pair.issued(id))
    }

    /// Write one new live token pair for grant `id`, inside the caller's
    /// transaction, and restart the grant's thirty days from `now`.
    ///
    /// `rotated_from` is the refresh hash the pair was minted from, `None`
    /// for a grant's first pair. It is what the grace rule asks about later:
    /// a token whose successor has itself been rotated is past its grace.
    ///
    /// Then the grant's token set is kept small: live tokens past their own
    /// thirty days go, the live ones past [`OAUTH_LIVE_TOKENS_KEPT`] (oldest
    /// first) are deleted outright, and only the newest
    /// [`OAUTH_RETIRED_TOKENS_KEPT`] rotated ones are remembered.
    async fn mint_oauth_token(
        &self,
        id: i64,
        pair: &TokenPair,
        rotated_from: Option<&str>,
        now: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO oauth_grant_token
                     (refresh_hash, grant_id, access_hash, access_expires_at,
                      refresh_expires_at, issued_at, rotated_at, rotated_from)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                vec![
                    Value::Text(token_hash(&pair.refresh_token)),
                    Value::Integer(id),
                    Value::Text(token_hash(&pair.access_token)),
                    Value::Integer(now + OAUTH_ACCESS_TTL_SECS),
                    Value::Integer(now + OAUTH_REFRESH_TTL_SECS),
                    Value::Integer(now),
                    rotated_from.map_or(Value::Null, |from| Value::Text(from.to_string())),
                ],
            )
            .await
            .context("writing an oauth token pair")?;
        self.conn
            .execute(
                "UPDATE oauth_grants SET refresh_expires_at = ?2 WHERE id = ?1",
                vec![
                    Value::Integer(id),
                    Value::Integer(now + OAUTH_REFRESH_TTL_SECS),
                ],
            )
            .await
            .context("restarting an oauth grant's refresh window")?;
        self.conn
            .execute(
                "DELETE FROM oauth_grant_token
                 WHERE grant_id = ?1 AND rotated_at IS NULL AND refresh_expires_at <= ?2",
                vec![Value::Integer(id), Value::Integer(now)],
            )
            .await
            .context("pruning expired oauth tokens")?;
        let live = self
            .grant_token_hashes(
                "SELECT refresh_hash FROM oauth_grant_token
                 WHERE grant_id = ?1 AND rotated_at IS NULL
                 ORDER BY issued_at DESC, rowid DESC",
                id,
            )
            .await?;
        let retired = self
            .grant_token_hashes(
                "SELECT refresh_hash FROM oauth_grant_token
                 WHERE grant_id = ?1 AND rotated_at IS NOT NULL
                 ORDER BY rotated_at DESC, rowid DESC",
                id,
            )
            .await?;
        let surplus = live
            .iter()
            .skip(OAUTH_LIVE_TOKENS_KEPT as usize)
            .chain(retired.iter().skip(OAUTH_RETIRED_TOKENS_KEPT as usize));
        for hash in surplus {
            self.conn
                .execute(
                    "DELETE FROM oauth_grant_token WHERE refresh_hash = ?1",
                    vec![Value::Text(hash.clone())],
                )
                .await
                .context("pruning old oauth tokens")?;
        }
        Ok(())
    }

    /// The refresh hashes one `SELECT refresh_hash ... WHERE grant_id = ?1`
    /// statement answers, in its order.
    async fn grant_token_hashes(&self, sql: &str, id: i64) -> Result<Vec<String>> {
        let mut rows = self
            .conn
            .query(sql, vec![Value::Integer(id)])
            .await
            .context("reading an oauth grant's tokens")?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .context("reading an oauth grant's tokens")?
        {
            if let Some(hash) = cell_text(&row, 0) {
                out.push(hash);
            }
        }
        Ok(out)
    }

    /// Present a refresh token: it rotates, it gets a pair of its own inside
    /// the grace window, it is a replay that revokes its grant, or it matches
    /// nothing. [`AuthStore::refresh_oauth_grant_at`] with the clock read
    /// now.
    pub async fn refresh_oauth_grant(
        &self,
        refresh_token: &str,
        client_id: &str,
        resource: Option<&str>,
    ) -> Result<RefreshOutcome> {
        self.refresh_oauth_grant_at(
            refresh_token,
            client_id,
            resource,
            chrono::Utc::now().timestamp(),
        )
        .await
    }

    /// [`AuthStore::refresh_oauth_grant`] at a given instant, unix seconds, so
    /// a test can step past the grace window without sleeping.
    ///
    /// Every grant holds a small set of refresh tokens in `oauth_grant_token`,
    /// each live or rotated, each with its own access token. The presented
    /// token decides, in this order:
    ///
    /// 1. **Live** for this client, inside its own and the grant's window, on
    ///    an enabled account: it is rotated. It is marked rotated now, its
    ///    access token dies on the spot, and a new pair is minted from it
    ///    ([`RefreshOutcome::Rotated`]).
    /// 2. **Rotated less than [`OAUTH_REFRESH_GRACE_SECS`] ago**, and no token
    ///    minted from it has itself been rotated since: it gets a new pair of
    ///    its own ([`RefreshOutcome::Reissued`]). The pair the first rotation
    ///    handed out stays live too, so two parallel sessions both keep
    ///    working. It still has to be this client's grant on an enabled
    ///    account inside its window, the same checks the live branch makes.
    /// 3. **Any other rotated token** is a replay and revokes the whole grant
    ///    ([`RefreshOutcome::Replayed`]): one that came back after the window,
    ///    one presented by another client, and one whose successor has been
    ///    rotated in its turn. The last is the token two rotations back: a
    ///    client that has already refreshed again from the answer does not
    ///    need the old token any more, so its return is evidence of a copy,
    ///    even inside the sixty seconds.
    /// 4. Anything else, a forgotten token among them, is
    ///    [`RefreshOutcome::Unknown`].
    ///
    /// Each new pair gets [`OAUTH_ACCESS_TTL_SECS`] for its access token and
    /// [`OAUTH_REFRESH_TTL_SECS`] for its refresh token from `now`, and the
    /// grant's window restarts with it, exactly as a rotation always did.
    ///
    /// All lookups and the writes they lead to are one `BEGIN IMMEDIATE`, so
    /// two requests racing one token are decided one after the other: the
    /// first rotates and the second, seeing the committed rotation, falls
    /// into the grace branch.
    ///
    /// The replay lookup keys on the token hash alone: a rotated token coming
    /// back late is evidence a copy of it leaked, whoever presents it.
    ///
    /// `resource` is what the client asked its next token to be minted for
    /// (RFC 8707 section 2.2), or `None` when it asked for nothing. It is
    /// compared against the audience the grant actually holds - never against
    /// the address this request arrived at - and a mismatch is
    /// [`RefreshOutcome::WrongTarget`] with nothing written.
    ///
    /// **The comparison is here rather than in the caller, and inside this
    /// transaction, because ordering is a security property.** A caller that
    /// read the grant first and refused on `resource` would refuse before the
    /// replay was ever looked for, and one extra form field would turn the
    /// tripwire off. So a replay is judged whatever resource was named, and
    /// only a token that would have been answered with a pair can be refused
    /// for its audience.
    pub async fn refresh_oauth_grant_at(
        &self,
        refresh_token: &str,
        client_id: &str,
        resource: Option<&str>,
        now: i64,
    ) -> Result<RefreshOutcome> {
        let presented = token_hash(refresh_token);
        let pair = TokenPair::new();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .context("refreshing an oauth grant")?;
        // Captured by the block below and read after `finish` commits, the
        // shape `rotate_mcp_token` already uses for a transaction whose caller
        // needs more out of it than `Result<()>`.
        let mut outcome = RefreshOutcome::Unknown;
        let result = async {
            let live = self
                .query_first(
                    "SELECT t.grant_id, g.resource
                     FROM oauth_grant_token t
                     JOIN oauth_grants g ON g.id = t.grant_id
                     JOIN users u ON u.name = g.user
                     WHERE t.refresh_hash = ?1 AND t.rotated_at IS NULL
                       AND g.client_id = ?2 AND g.refresh_expires_at > ?3
                       AND t.refresh_expires_at > ?3 AND u.disabled = 0",
                    vec![
                        Value::Text(presented.clone()),
                        Value::Text(client_id.to_string()),
                        Value::Integer(now),
                    ],
                )
                .await?;
            if let Some(row) = live {
                let Ok(Value::Integer(id)) = row.get_value(0) else {
                    return Ok(());
                };
                if !audience_matches(&row, 1, resource)? {
                    outcome = RefreshOutcome::WrongTarget;
                    return Ok(());
                }
                self.conn
                    .execute(
                        "UPDATE oauth_grant_token SET rotated_at = ?2, access_hash = NULL
                         WHERE refresh_hash = ?1",
                        vec![Value::Text(presented.clone()), Value::Integer(now)],
                    )
                    .await
                    .context("rotating an oauth grant")?;
                self.mint_oauth_token(id, &pair, Some(&presented), now)
                    .await?;
                outcome = RefreshOutcome::Rotated(pair.issued(id));
                return Ok(());
            }
            let rotated = self
                .query_first(
                    "SELECT grant_id, rotated_at FROM oauth_grant_token
                     WHERE refresh_hash = ?1 AND rotated_at IS NOT NULL",
                    vec![Value::Text(presented.clone())],
                )
                .await?;
            let Some(row) = rotated else {
                return Ok(());
            };
            // Fails closed: a grant this store may have to revoke must not be
            // left alive by a row it could not read. The whole transaction
            // rolls back and the caller sees the error.
            let (Ok(Value::Integer(id)), Ok(Value::Integer(rotated_at))) =
                (row.get_value(0), row.get_value(1))
            else {
                bail!("an oauth token row is unreadable");
            };
            let since_rotation = now - rotated_at;
            let successor_rotated = self
                .query_first(
                    "SELECT 1 FROM oauth_grant_token
                     WHERE rotated_from = ?1 AND rotated_at IS NOT NULL",
                    vec![Value::Text(presented.clone())],
                )
                .await?
                .is_some();
            if since_rotation < OAUTH_REFRESH_GRACE_SECS && !successor_rotated {
                let grant = self
                    .query_first(
                        "SELECT g.client_id, g.resource, g.refresh_expires_at, u.disabled
                         FROM oauth_grants g JOIN users u ON u.name = g.user
                         WHERE g.id = ?1",
                        vec![Value::Integer(id)],
                    )
                    .await?;
                let Some(grant) = grant else {
                    return Ok(());
                };
                // Another client presenting it is not a parallel session of
                // this one: that stays a replay, below.
                if cell_text(&grant, 0).as_deref() == Some(client_id) {
                    let open = matches!(grant.get_value(2), Ok(Value::Integer(at)) if at > now);
                    let enabled = matches!(grant.get_value(3), Ok(Value::Integer(0)));
                    if !open || !enabled {
                        return Ok(());
                    }
                    if !audience_matches(&grant, 1, resource)? {
                        outcome = RefreshOutcome::WrongTarget;
                        return Ok(());
                    }
                    self.mint_oauth_token(id, &pair, Some(&presented), now)
                        .await?;
                    outcome = RefreshOutcome::Reissued {
                        grant: pair.issued(id),
                        since_rotation,
                    };
                    return Ok(());
                }
            }
            self.end_oauth_grant(id, OauthGrantEndReason::Replayed, now)
                .await
                .context("revoking a replayed oauth grant")?;
            outcome = RefreshOutcome::Replayed {
                grant: id,
                since_rotation,
            };
            Ok(())
        }
        .await;
        self.finish(result).await?;
        // The lines are emitted after the commit, never inside the
        // transaction body. A rollback - a disk error, a busy timeout against
        // the CLI holding the write lock - must not leave an audit record
        // claiming a revocation that did not happen. Seconds since rotation
        // ride both INFO lines, so the grace window can be tuned from a real
        // deployment's log. No token material, ever.
        match &outcome {
            RefreshOutcome::Rotated(grant) => {
                tracing::debug!(grant = grant.id, client = %client_id, "rotated an oauth grant");
            }
            RefreshOutcome::Reissued {
                grant,
                since_rotation,
            } => tracing::info!(
                grant = grant.id,
                client = %client_id,
                since_rotation_secs = *since_rotation,
                "a refresh token reused inside the grace window got its own pair"
            ),
            RefreshOutcome::Replayed {
                grant,
                since_rotation,
            } => tracing::info!(
                grant = *grant,
                client = %client_id,
                since_rotation_secs = *since_rotation,
                "a replayed refresh token revoked an oauth grant"
            ),
            // Neither is worth an audit line: a miss is a miss, and a wrong
            // audience is a protocol mistake the route already logs one WARN
            // for, with the client id beside it.
            RefreshOutcome::Unknown | RefreshOutcome::WrongTarget => {}
        }
        Ok(outcome)
    }

    /// End grant `id` inside the caller's transaction: write the record of
    /// why, then delete the grant and every token it holds.
    ///
    /// The record copies the client's name and redirect host instead of
    /// pointing at the registration, because the client prune may collect
    /// that registration as soon as its last grant is gone, and the person
    /// still needs to recognise the client. The account's records are kept
    /// to [`OAUTH_ENDED_KEPT_SECS`] and [`OAUTH_ENDED_KEPT_PER_ACCOUNT`] here,
    /// at the one place they grow.
    ///
    /// Answers whether a grant was there to end.
    async fn end_oauth_grant(
        &self,
        id: i64,
        reason: OauthGrantEndReason,
        now: i64,
    ) -> Result<bool> {
        let Some(row) = self
            .query_first(
                "SELECT g.user, g.client_id, c.client_name, c.redirect_uris,
                        g.created_at, g.last_used
                 FROM oauth_grants g
                 LEFT JOIN oauth_clients c ON c.client_id = g.client_id
                 WHERE g.id = ?1",
                vec![Value::Integer(id)],
            )
            .await?
        else {
            return Ok(false);
        };
        let user = cell_text(&row, 0).unwrap_or_default();
        let redirect_host = decode_redirect_uris(cell_text(&row, 3).as_deref())
            .first()
            .map(|uri| redirect_host(uri))
            .unwrap_or_default();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO oauth_grant_ended
                     (grant_id, user, client_id, client_name, redirect_host,
                      created_at, last_used, ended_at, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                vec![
                    Value::Integer(id),
                    Value::Text(user.clone()),
                    Value::Text(cell_text(&row, 1).unwrap_or_default()),
                    Value::Text(
                        cell_text(&row, 2).unwrap_or_else(|| GONE_OAUTH_CLIENT.to_string()),
                    ),
                    Value::Text(redirect_host),
                    Value::Text(cell_text(&row, 4).unwrap_or_default()),
                    cell_text(&row, 5).map_or(Value::Null, Value::Text),
                    Value::Integer(now),
                    Value::Text(reason.as_str().to_string()),
                ],
            )
            .await
            .context("recording an ended oauth grant")?;
        self.conn
            .execute(
                "DELETE FROM oauth_grant_token WHERE grant_id = ?1",
                vec![Value::Integer(id)],
            )
            .await
            .context("deleting an oauth grant's tokens")?;
        self.conn
            .execute(
                "DELETE FROM oauth_grants WHERE id = ?1",
                vec![Value::Integer(id)],
            )
            .await
            .context("deleting an oauth grant")?;
        self.conn
            .execute(
                "DELETE FROM oauth_grant_ended WHERE user = ?1 AND ended_at <= ?2",
                vec![
                    Value::Text(user.clone()),
                    Value::Integer(now - OAUTH_ENDED_KEPT_SECS),
                ],
            )
            .await
            .context("pruning ended oauth grant records")?;
        let mut rows = self
            .conn
            .query(
                "SELECT grant_id FROM oauth_grant_ended WHERE user = ?1
                 ORDER BY ended_at DESC, grant_id DESC",
                vec![Value::Text(user)],
            )
            .await
            .context("reading ended oauth grant records")?;
        let mut surplus = Vec::new();
        let mut seen = 0;
        while let Some(row) = rows
            .next()
            .await
            .context("reading ended oauth grant records")?
        {
            seen += 1;
            if seen > OAUTH_ENDED_KEPT_PER_ACCOUNT
                && let Ok(Value::Integer(old)) = row.get_value(0)
            {
                surplus.push(old);
            }
        }
        drop(rows);
        for old in surplus {
            self.conn
                .execute(
                    "DELETE FROM oauth_grant_ended WHERE grant_id = ?1",
                    vec![Value::Integer(old)],
                )
                .await
                .context("pruning ended oauth grant records")?;
        }
        Ok(true)
    }

    /// Resolve an OAuth access token to its account, for one resource. `None`
    /// for an unknown, expired, revoked or wrong-audience token and for a
    /// disabled account - deliberately indistinguishable, the way
    /// [`AuthStore::mcp_token_user`] answers, because the gate sends one
    /// refusal for all of them.
    ///
    /// The token is looked up among the grant's live pairs in
    /// `oauth_grant_token`; a rotated pair has no access hash left, so its
    /// access token stopped at the rotation.
    ///
    /// `resource` is the origin this request arrived at. It is compared to the
    /// one the grant was issued for after [`normalize_resource`] has taken one
    /// trailing slash off each side, and the comparison happens *in the
    /// statement*: a token minted for another deployment of this server cannot
    /// be replayed here, and no branch in Rust can forget to check.
    ///
    /// Stamps `last_used` on a hit, and prunes any grant whose account is gone
    /// (and the tokens of any grant that is gone) on every call that reaches
    /// the statement - the same defense in depth `mcp_token_user` applies to
    /// tokens, for a row reached by some path the issuing transaction does not
    /// cover. A call naming an empty resource returns before it, since such a
    /// call could never have matched a grant anyway.
    pub async fn oauth_access_user(&self, token: &str, resource: &str) -> Result<Option<User>> {
        let hash = token_hash(token);
        let resource = normalize_resource(resource);
        // See `issue_oauth_grant`: nothing opens the empty resource, whatever
        // a row happens to hold.
        if resource.is_empty() {
            return Ok(None);
        }
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "DELETE FROM oauth_grants
                 WHERE NOT EXISTS (SELECT 1 FROM users u WHERE u.name = oauth_grants.user)",
                (),
            )
            .await
            .context("pruning orphaned oauth grants")?;
        self.conn
            .execute(
                "DELETE FROM oauth_grant_token
                 WHERE NOT EXISTS
                     (SELECT 1 FROM oauth_grants g WHERE g.id = oauth_grant_token.grant_id)",
                (),
            )
            .await
            .context("pruning orphaned oauth tokens")?;
        let Some(row) = self
            .query_first(
                &format!(
                    "SELECT {USER_COLUMNS_JOINED}, g.id
                     FROM oauth_grant_token t
                     JOIN oauth_grants g ON g.id = t.grant_id
                     JOIN users u ON u.name = g.user
                     WHERE t.access_hash = ?1 AND t.access_expires_at > ?2 AND g.resource = ?3"
                ),
                vec![
                    Value::Text(hash),
                    Value::Integer(now),
                    Value::Text(resource),
                ],
            )
            .await?
        else {
            return Ok(None);
        };
        let user = user_from_row(&row);
        if user.disabled {
            return Ok(None);
        }
        if let Ok(Value::Integer(id)) = row.get_value(7) {
            self.conn
                .execute(
                    "UPDATE oauth_grants SET last_used = ?2 WHERE id = ?1",
                    vec![
                        Value::Integer(id),
                        Value::Text(chrono::Utc::now().to_rfc3339()),
                    ],
                )
                .await
                .context("stamping an oauth grant's last_used")?;
        }
        Ok(Some(user))
    }

    /// Every OAuth grant `user` holds, newest first, never carrying a token.
    ///
    /// A `LEFT JOIN` on purpose: a grant whose registration vanished still
    /// resolves its tokens, so it has to stay listed to stay revocable. It
    /// shows `GONE_OAUTH_CLIENT` where the name would be rather than
    /// dropping out of the one screen that can stop it.
    ///
    /// A grant past its refresh window is left out, in the statement: it can
    /// no longer work by any path, so listing it would show a person a
    /// connection they cannot act on and cannot tell from a live one.
    /// [`AuthStore::prune_oauth_grants`] removes the rows; this is what keeps
    /// the card honest in between. The record of grants that ended is
    /// [`AuthStore::list_ended_oauth_grants`].
    pub async fn list_oauth_grants(&self, user: &str) -> Result<Vec<OauthGrantInfo>> {
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT g.id, g.client_id, c.client_name, c.redirect_uris,
                        g.created_at, g.last_used, g.refresh_expires_at
                 FROM oauth_grants g
                 LEFT JOIN oauth_clients c ON c.client_id = g.client_id
                 WHERE g.user = ?1 AND g.refresh_expires_at > ?2
                 ORDER BY g.created_at DESC, g.id DESC",
                vec![
                    Value::Text(user.clone()),
                    Value::Integer(chrono::Utc::now().timestamp()),
                ],
            )
            .await
            .with_context(|| format!("listing oauth grants for user '{user}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing oauth grants for user '{user}'"))?
        {
            let Ok(Value::Integer(id)) = row.get_value(0) else {
                continue;
            };
            let refresh_expires_at = match row.get_value(6) {
                Ok(Value::Integer(secs)) => rfc3339_from_unix(secs),
                _ => String::new(),
            };
            out.push(OauthGrantInfo {
                id,
                client_id: cell_text(&row, 1).unwrap_or_default(),
                client_name: cell_text(&row, 2).unwrap_or_else(|| GONE_OAUTH_CLIENT.to_string()),
                redirect_host: decode_redirect_uris(cell_text(&row, 3).as_deref())
                    .first()
                    .map(|uri| redirect_host(uri))
                    .unwrap_or_default(),
                created_at: cell_text(&row, 4).unwrap_or_default(),
                last_used: cell_text(&row, 5),
                refresh_expires_at,
                ended: None,
            });
        }
        Ok(out)
    }

    /// The grants of `user` that ended in the last [`OAUTH_ENDED_KEPT_SECS`],
    /// newest first, each with [`OauthGrantInfo::ended`] set to when and why.
    /// At most [`OAUTH_ENDED_KEPT_PER_ACCOUNT`] of them.
    ///
    /// So a person whose client was signed out can read why on the Connected
    /// clients card instead of finding the row gone. The fields are the ones
    /// the grant had when it ended; `refresh_expires_at` holds the moment it
    /// ended, since nothing refreshes it any more.
    pub async fn list_ended_oauth_grants(&self, user: &str) -> Result<Vec<OauthGrantInfo>> {
        self.list_ended_oauth_grants_at(user, chrono::Utc::now().timestamp())
            .await
    }

    /// [`AuthStore::list_ended_oauth_grants`] at a given instant, unix
    /// seconds, for a test that steps past the seven days.
    pub async fn list_ended_oauth_grants_at(
        &self,
        user: &str,
        now: i64,
    ) -> Result<Vec<OauthGrantInfo>> {
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT grant_id, client_id, client_name, redirect_host, created_at,
                        last_used, ended_at, reason
                 FROM oauth_grant_ended
                 WHERE user = ?1 AND ended_at > ?2
                 ORDER BY ended_at DESC, grant_id DESC
                 LIMIT ?3",
                vec![
                    Value::Text(user.clone()),
                    Value::Integer(now - OAUTH_ENDED_KEPT_SECS),
                    Value::Integer(OAUTH_ENDED_KEPT_PER_ACCOUNT),
                ],
            )
            .await
            .with_context(|| format!("listing ended oauth grants for user '{user}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing ended oauth grants for user '{user}'"))?
        {
            let (Ok(Value::Integer(id)), Ok(Value::Integer(ended_at))) =
                (row.get_value(0), row.get_value(6))
            else {
                continue;
            };
            let Some(reason) = cell_text(&row, 7)
                .as_deref()
                .and_then(OauthGrantEndReason::from_db)
            else {
                continue;
            };
            let at = rfc3339_from_unix(ended_at);
            out.push(OauthGrantInfo {
                id,
                client_id: cell_text(&row, 1).unwrap_or_default(),
                client_name: cell_text(&row, 2).unwrap_or_default(),
                redirect_host: cell_text(&row, 3).unwrap_or_default(),
                created_at: cell_text(&row, 4).unwrap_or_default(),
                last_used: cell_text(&row, 5),
                refresh_expires_at: at.clone(),
                ended: Some(OauthGrantEnded { at, reason }),
            });
        }
        Ok(out)
    }

    /// Revoke one of `user`'s grants by id, reporting whether a row went.
    /// `false` covers both an unknown id and one belonging to another account,
    /// deliberately indistinguishable so a caller cannot probe for ids.
    ///
    /// One transaction stops every token of the grant at once and writes the
    /// record of why ([`OauthGrantEndReason::Revoked`]), which the Connected
    /// clients card lists for a while.
    pub async fn revoke_oauth_grant(&self, user: &str, id: i64) -> Result<bool> {
        let user = normalize_account_name(user)?;
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("revoking an oauth grant for user '{user}'"))?;
        let result = async {
            if self
                .query_first(
                    "SELECT 1 FROM oauth_grants WHERE id = ?1 AND user = ?2",
                    vec![Value::Integer(id), Value::Text(user.clone())],
                )
                .await?
                .is_none()
            {
                return Ok(false);
            }
            self.end_oauth_grant(id, OauthGrantEndReason::Revoked, now)
                .await
        }
        .await;
        let changed = self
            .finish(result)
            .await
            .with_context(|| format!("revoking an oauth grant for user '{user}'"))?;
        if changed {
            tracing::info!(grant = id, user = %user, "revoked an oauth grant");
        }
        Ok(changed)
    }

    /// Revoke the grant `token` belongs to - the access token or the refresh
    /// token of any of its live pairs - when it was issued to `client_id`,
    /// reporting whether a grant went.
    ///
    /// For RFC 7009 revocation, where a public client presents a token and its
    /// own `client_id` and nothing else. The client must match, so a leaked
    /// token cannot be revoked by a stranger's registration; the endpoint
    /// answers the same either way, so a caller learns nothing from it. The
    /// record of why is [`OauthGrantEndReason::ClientRevoked`].
    pub async fn revoke_oauth_grant_by_token(&self, token: &str, client_id: &str) -> Result<bool> {
        let hash = token_hash(token);
        let now = chrono::Utc::now().timestamp();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .context("revoking an oauth grant by its token")?;
        let result = async {
            let Some(row) = self
                .query_first(
                    "SELECT t.grant_id FROM oauth_grant_token t
                     JOIN oauth_grants g ON g.id = t.grant_id
                     WHERE (t.access_hash = ?1 OR t.refresh_hash = ?1)
                       AND t.rotated_at IS NULL AND g.client_id = ?2",
                    vec![Value::Text(hash), Value::Text(client_id.to_string())],
                )
                .await?
            else {
                return Ok(false);
            };
            let Ok(Value::Integer(id)) = row.get_value(0) else {
                bail!("an oauth token row has an unreadable grant id");
            };
            self.end_oauth_grant(id, OauthGrantEndReason::ClientRevoked, now)
                .await
        }
        .await;
        let changed = self
            .finish(result)
            .await
            .context("revoking an oauth grant by its token")?;
        if changed {
            tracing::info!(client_id, "revoked an oauth grant by its token");
        }
        Ok(changed)
    }

    /// Drop every OAuth grant of one account, with its tokens and the record
    /// of its ended grants. Called by both removal paths inside their
    /// transaction, for the reason [`AuthStore::delete_mcp_tokens_of`]
    /// documents one table over: `oauth_grants` carries no foreign key, so a
    /// row that outlived its account would resolve for whoever next claims the
    /// freed name.
    ///
    /// Deliberately not called by [`AuthStore::set_disabled`]: disabling is
    /// reversible, and both OAuth lookups refuse a disabled account at read
    /// time, so re-enabling hands the connections back rather than making
    /// every client authorize again.
    ///
    /// The registrations themselves are left alone: they are shared, and the
    /// ones nobody is connected through go on their own schedule, through
    /// [`AuthStore::prune_oauth_clients`].
    async fn delete_oauth_grants_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM oauth_grant_token WHERE grant_id IN
                     (SELECT id FROM oauth_grants WHERE user = ?1)",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing oauth tokens for user '{name}'"))?;
        self.conn
            .execute(
                "DELETE FROM oauth_grants WHERE user = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing oauth grants for user '{name}'"))?;
        self.conn
            .execute(
                "DELETE FROM oauth_grant_ended WHERE user = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing ended oauth grants for user '{name}'"))?;
        Ok(())
    }

    /// The visibility record of one domain: `Some` when it is private, `None`
    /// when it is shared, which is every domain nobody ever made private.
    ///
    /// The lookup is exact on the trimmed name (`normalize_domain`), so it
    /// answers for the same string the engine keys its domain map on.
    pub async fn domain_visibility(&self, domain: &str) -> Result<Option<DomainAcl>> {
        let domain = normalize_domain(domain)?;
        let _guard = self.guard.lock().await;
        self.acl_of(&domain).await
    }

    /// [`AuthStore::domain_visibility`] without the lock or the folding, for
    /// the methods that already hold both.
    async fn acl_of(&self, domain: &str) -> Result<Option<DomainAcl>> {
        let row = self
            .query_first(
                "SELECT domain, owner FROM domain_acl WHERE domain = ?1",
                vec![Value::Text(domain.to_string())],
            )
            .await
            .with_context(|| format!("reading the visibility of domain '{domain}'"))?;
        // The name comes from the caller rather than from the row: this is a
        // lookup by exact name, so the row's own copy can only agree, and
        // defaulting an unreadable cell to `""` here would hand back an acl
        // that names a domain nobody asked about. The owner keeps its default
        // because `""` is the one value no live account can match, so an
        // unreadable owner cell reads as owned by nobody - closed, not open.
        Ok(row.map(|row| DomainAcl {
            domain: domain.to_string(),
            owner: cell_text(&row, 1).unwrap_or_default(),
        }))
    }

    /// Make `domain` private, owned by `owner`, or make it shared again.
    ///
    /// `private = true` on a SHARED domain writes the acl row naming `owner`
    /// and leaves the (nonexistent) membership alone. The one row it drops is
    /// the new owner's own membership, if some earlier private spell left one:
    /// an owner holds every level, so the row could only say less than the
    /// truth, and it would come back to life the moment the domain is handed
    /// on again. This is the same step [`AuthStore::transfer_domain`] takes,
    /// for the same reason - the two paths that change an owner must agree.
    /// On a domain that is already private this writes nothing at all; see the
    /// paragraph below. `private = false` deletes the acl row *and*
    /// every membership row for the domain: membership only means anything
    /// while a domain is private, and leaving the rows behind would silently
    /// restore them if the domain were ever made private again by somebody
    /// else.
    ///
    /// `owner` must name an existing, enabled account. An acl row pointing at
    /// nobody would be a domain only an admin could ever reach, with no way to
    /// invite anyone into it, which is a state no caller can have meant.
    ///
    /// Both halves run in one `BEGIN IMMEDIATE` transaction, for the reason
    /// [`AuthStore::issue_mcp_token`] documents: the owner check and the write
    /// must see the same users table, and the two deletes must not be
    /// separable by another writer.
    ///
    /// Turning privacy off on a domain that was never private is a no-op
    /// rather than an error - the caller asked for a state that already holds.
    ///
    /// **Privatizing an already-private domain is a no-op too, and that is the
    /// half that matters.** Writing the row again would delete the acl row and
    /// insert one naming the CALLER, so a retried request, a stale client or a
    /// script would hand the domain to whoever asked last and drop its previous
    /// owner - who holds no membership row by construction - to no access at
    /// all, with nothing said to either of them. A `PUT` states a visibility;
    /// changing an owner is [`AuthStore::transfer_domain`]'s job, and it is a
    /// verb of its own for exactly this reason. So the record is read first and
    /// left exactly as it stands, owner and members included, and the answer
    /// says it was already private so a caller can say so too.
    pub async fn set_domain_visibility(
        &self,
        domain: &str,
        private: bool,
        owner: &str,
    ) -> Result<VisibilityWrite> {
        self.write_visibility(domain, private, owner, true).await
    }

    /// Make a domain whose access follows GitHub private, recording `set_by`
    /// as the one who chose it: the visibility half of connecting a GitHub
    /// domain with readers `private` on an instance with a GitHub App.
    ///
    /// The same write as [`AuthStore::set_domain_visibility`] with `private =
    /// true` (an already-private domain is left exactly as it stands), with
    /// one difference: `set_by` may be the reserved machine owner's name,
    /// [`crate::scope::OWNER_IDENTITY_NAME`], which no account holds, so the
    /// live-account check is skipped for it. That is safe here and nowhere
    /// else: on a governed domain the row means only "private", and its
    /// `owner` and every membership row are never read (`decide_governed`
    /// in the scope module). Any other `set_by` must still name a live
    /// account.
    pub async fn set_governed_private(
        &self,
        domain: &str,
        set_by: &str,
    ) -> Result<VisibilityWrite> {
        let is_owner = normalize_account_name(set_by)? == crate::scope::OWNER_IDENTITY_NAME;
        self.write_visibility(domain, true, set_by, !is_owner).await
    }

    /// Delete the acl row of `domain` and nothing else: the undo of
    /// [`AuthStore::set_governed_private`] for a connect whose registration
    /// failed after the row was written. No membership row is touched, so it
    /// can never drop a member somebody else invited. Answers whether there
    /// was a row.
    pub async fn take_back_governed_private(&self, domain: &str) -> Result<bool> {
        let domain = normalize_domain(domain)?;
        let _guard = self.guard.lock().await;
        let had = self.acl_of(&domain).await?.is_some();
        self.conn
            .execute(
                "DELETE FROM domain_acl WHERE domain = ?1",
                vec![Value::Text(domain.clone())],
            )
            .await
            .with_context(|| format!("taking back the private record of domain '{domain}'"))?;
        Ok(had)
    }

    /// The write behind [`AuthStore::set_domain_visibility`] and
    /// [`AuthStore::set_governed_private`]; `check_owner` is whether a
    /// privatizing write requires `owner` to be a live account.
    async fn write_visibility(
        &self,
        domain: &str,
        private: bool,
        owner: &str,
        check_owner: bool,
    ) -> Result<VisibilityWrite> {
        let domain = normalize_domain(domain)?;
        let owner = normalize_account_name(owner)?;
        let now = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("setting the visibility of domain '{domain}'"))?;
        let result = async {
            if !private {
                self.delete_members_of_domain(&domain).await?;
                self.conn
                    .execute(
                        "DELETE FROM domain_acl WHERE domain = ?1",
                        vec![Value::Text(domain.clone())],
                    )
                    .await
                    .with_context(|| format!("making domain '{domain}' shared"))?;
                return Ok(VisibilityWrite::Written);
            }
            if let Some(acl) = self.acl_of(&domain).await? {
                return Ok(VisibilityWrite::AlreadyPrivate { owner: acl.owner });
            }
            if check_owner {
                self.require_live_user(&owner).await?;
            }
            // Delete then insert rather than an upsert clause: two plain
            // statements inside the transaction that already serializes them,
            // with no dependence on which conflict syntax the embedded
            // database supports.
            self.conn
                .execute(
                    "DELETE FROM domain_acl WHERE domain = ?1",
                    vec![Value::Text(domain.clone())],
                )
                .await
                .with_context(|| format!("making domain '{domain}' private"))?;
            self.conn
                .execute(
                    "DELETE FROM domain_member
                     WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(PRINCIPAL_USER.to_string()),
                        Value::Text(owner.clone()),
                    ],
                )
                .await
                .with_context(|| format!("making domain '{domain}' private"))?;
            self.conn
                .execute(
                    "INSERT INTO domain_acl (domain, visibility, owner, updated_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(VISIBILITY_PRIVATE.to_string()),
                        Value::Text(owner.clone()),
                        Value::Text(now.clone()),
                    ],
                )
                .await
                .with_context(|| format!("making domain '{domain}' private"))?;
            Ok(VisibilityWrite::Written)
        }
        .await;
        self.finish(result).await
    }

    /// Retire every visibility and membership record of one domain, for a
    /// domain that has just been unregistered.
    ///
    /// Deliberately not [`AuthStore::set_domain_visibility`] with `private =
    /// false`: that means "this domain is shared now", takes an owner it has no
    /// use for here, and leaves a domain standing. This means "there is no such
    /// domain any more", and it is the only caller that is allowed to drop an
    /// acl row without somebody deciding the domain should be public.
    ///
    /// Both tables go in one transaction, so a domain can never be left
    /// half-forgotten: an acl row with no members would still hide the name
    /// from everyone but an admin. Answers whether there was an acl row at all,
    /// which is how a caller can tell a private domain's records from a shared
    /// domain's absence of them.
    ///
    /// Its Reflection collaborators go too, in the same transaction.
    ///
    /// Write-list entries naming the domain are left alone: they match nothing
    /// until a domain of that name exists again.
    pub async fn forget_domain(&self, domain: &str) -> Result<bool> {
        let domain = normalize_domain(domain)?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("forgetting the records of domain '{domain}'"))?;
        let result = async {
            let was_private = self.acl_of(&domain).await?.is_some();
            self.delete_members_of_domain(&domain).await?;
            self.conn
                .execute(
                    "DELETE FROM domain_reflection_grant WHERE domain = ?1",
                    vec![Value::Text(domain.clone())],
                )
                .await
                .with_context(|| format!("forgetting the records of domain '{domain}'"))?;
            self.conn
                .execute(
                    "DELETE FROM domain_acl WHERE domain = ?1",
                    vec![Value::Text(domain.clone())],
                )
                .await
                .with_context(|| format!("forgetting the records of domain '{domain}'"))?;
            Ok(was_private)
        }
        .await;
        self.finish(result).await
    }

    /// Carry every record of domain `old` over to `new`: its `domain_acl` row,
    /// its `domain_member` rows, its `overlay_grant` rows (the draft
    /// share-links, revoked ones included, so the history stays with the
    /// domain) and its Reflection collaborators, in one transaction. For a domain rename, which has already
    /// decided the name changes; this decides nothing.
    ///
    /// The renamed domain's records win. Rows already under `new` while `old`
    /// still has records can only be stale ones - left by a removal that never
    /// forgot them, since this call is one transaction and never leaves rows
    /// under both names - so every one of them is dropped: a stale owner,
    /// member or share-link must never gain access to the renamed domain.
    /// Names compare byte for byte, so a rename that changes only the case
    /// moves the rows too.
    ///
    /// Idempotent, for a rename completed again after a crash: with nothing
    /// under `old` the call changes nothing, and in particular leaves the rows
    /// under `new` alone, since those are the ones an earlier run moved.
    ///
    /// Exact write-list entries naming `old` are rewritten to `new` as well;
    /// `*` entries are left alone (a prefix names a family of domains, not
    /// this one), and an account that already named `new` keeps one entry.
    pub async fn rename_domain(&self, old: &str, new: &str) -> Result<()> {
        let old = normalize_domain(old)?;
        let new = normalize_domain(new)?;
        if old == new {
            return Ok(());
        }
        let _guard = self.guard.lock().await;
        let context = || format!("moving the records of domain '{old}' to '{new}'");
        self.begin_immediate().await.with_context(context)?;
        let names = vec![Value::Text(old.clone()), Value::Text(new.clone())];
        let result = async {
            // The write lists first, and before the early return below: a
            // shared domain has no acl, member or grant row, so the probe
            // would skip it. Only exact entries name a domain (a domain name
            // never holds a `*`), so a prefix entry is never touched. An entry
            // already naming `new` is a real entry, not a stale row, and is
            // kept: the account's `old` row is dropped instead of colliding
            // with it on the primary key.
            let mut holders = Vec::new();
            {
                let mut rows = self
                    .conn
                    .query(
                        "SELECT account FROM account_write_domain WHERE pattern = ?1",
                        vec![Value::Text(old.clone())],
                    )
                    .await
                    .with_context(context)?;
                while let Some(row) = rows.next().await.with_context(context)? {
                    if let Some(account) = cell_text(&row, 0) {
                        holders.push(account);
                    }
                }
            }
            for account in holders {
                let both = self
                    .query_first(
                        "SELECT 1 FROM account_write_domain WHERE account = ?1 AND pattern = ?2",
                        vec![Value::Text(account.clone()), Value::Text(new.clone())],
                    )
                    .await
                    .with_context(context)?
                    .is_some();
                let sql = if both {
                    "DELETE FROM account_write_domain WHERE account = ?1 AND pattern = ?2"
                } else {
                    "UPDATE account_write_domain SET pattern = ?3 WHERE account = ?1 AND pattern = ?2"
                };
                let params = if both {
                    vec![Value::Text(account), Value::Text(old.clone())]
                } else {
                    vec![
                        Value::Text(account),
                        Value::Text(old.clone()),
                        Value::Text(new.clone()),
                    ]
                };
                self.conn.execute(sql, params).await.with_context(context)?;
            }
            let mut old_has_records = false;
            for sql in [
                "SELECT 1 FROM domain_acl WHERE domain = ?1",
                "SELECT 1 FROM domain_member WHERE domain = ?1",
                "SELECT 1 FROM overlay_grant WHERE domain = ?1",
                // A shared domain's grants are its only rows here.
                "SELECT 1 FROM domain_reflection_grant WHERE domain = ?1",
            ] {
                if self
                    .query_first(sql, vec![Value::Text(old.clone())])
                    .await
                    .with_context(context)?
                    .is_some()
                {
                    old_has_records = true;
                    break;
                }
            }
            if !old_has_records {
                return Ok(());
            }
            // Clear the stale rows first, then move: plain statements rather
            // than an upsert clause, for the reason `set_domain_visibility`
            // gives.
            for sql in [
                "DELETE FROM domain_acl WHERE domain = ?2",
                "DELETE FROM domain_member WHERE domain = ?2",
                "DELETE FROM overlay_grant WHERE domain = ?2",
                "DELETE FROM domain_reflection_grant WHERE domain = ?2",
                "UPDATE domain_acl SET domain = ?2 WHERE domain = ?1",
                "UPDATE domain_member SET domain = ?2 WHERE domain = ?1",
                "UPDATE overlay_grant SET domain = ?2 WHERE domain = ?1",
                "UPDATE domain_reflection_grant SET domain = ?2 WHERE domain = ?1",
            ] {
                self.conn
                    .execute(sql, names.clone())
                    .await
                    .with_context(context)?;
            }
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Hand a private domain to a different owner.
    ///
    /// The new owner must be an existing, enabled account, and the domain must
    /// already be private: there is no owner to transfer on a shared domain,
    /// and inventing one here would make a domain private as a side effect of
    /// a transfer.
    ///
    /// A membership row for the new owner is dropped in the same transaction.
    /// The owner already holds every level (see [`DomainRight`] in
    /// `crate::scope`), so leaving one behind would be a row that says less
    /// than the truth and would come back to life the moment the domain is
    /// transferred away again.
    ///
    /// [`DomainRight`]: crate::scope::DomainRight
    pub async fn transfer_domain(&self, domain: &str, new_owner: &str) -> Result<()> {
        let domain = normalize_domain(domain)?;
        let new_owner = normalize_account_name(new_owner)?;
        let now = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("transferring domain '{domain}'"))?;
        let result = async {
            if self.acl_of(&domain).await?.is_none() {
                return Err(refuse(
                    RefusalKind::NotPrivate,
                    format!("domain '{domain}' is not private, so it has no owner to transfer"),
                ));
            }
            self.require_live_user(&new_owner).await?;
            self.conn
                .execute(
                    "UPDATE domain_acl SET owner = ?2, updated_at = ?3 WHERE domain = ?1",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(new_owner.clone()),
                        Value::Text(now.clone()),
                    ],
                )
                .await
                .with_context(|| format!("transferring domain '{domain}'"))?;
            self.conn
                .execute(
                    "DELETE FROM domain_member
                     WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(PRINCIPAL_USER.to_string()),
                        Value::Text(new_owner.clone()),
                    ],
                )
                .await
                .with_context(|| format!("transferring domain '{domain}'"))?;
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Everyone invited to `domain`, by name. The owner is deliberately absent:
    /// it is a property of the domain, not a membership row, and it is read
    /// from [`AuthStore::domain_visibility`].
    pub async fn domain_members(&self, domain: &str) -> Result<Vec<DomainMember>> {
        let domain = normalize_domain(domain)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT principal, level, added_by, added_at, reflection FROM domain_member
                 WHERE domain = ?1 AND principal_kind = ?2 ORDER BY principal",
                vec![
                    Value::Text(domain.clone()),
                    Value::Text(PRINCIPAL_USER.to_string()),
                ],
            )
            .await
            .with_context(|| format!("listing the members of domain '{domain}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing the members of domain '{domain}'"))?
        {
            out.push(DomainMember {
                principal: cell_text(&row, 0).unwrap_or_default(),
                level: member_level_from_db(&cell_text(&row, 1).unwrap_or_default()),
                added_by: cell_text(&row, 2).unwrap_or_default(),
                added_at: cell_text(&row, 3).unwrap_or_default(),
                reflection: reflection_level_from_db(cell_text(&row, 4)),
            });
        }
        Ok(out)
    }

    /// Invite `principal` to `domain` at `level`, or change the level it is
    /// already there at. `reflection` is the member's Reflection level there;
    /// `None` keeps the stored one of an existing row and means
    /// [`ReflectionLevel::Default`] for a new one, so changing one axis never
    /// resets the other. `added_by` is recorded as given (trimmed): it is an
    /// audit field, usually the acting account's name, and it is not resolved
    /// against the users table because a non-account actor may legitimately
    /// grant membership. It may not be empty, though - a blank "invited by" is
    /// not an audit trail, and it is what a caller that forgot to pass one
    /// would write.
    ///
    /// Four things are refused, all inside the one transaction that also does
    /// the write so none of them can be raced past:
    ///
    /// * a domain that is not private, because a membership row there would
    ///   grant nothing and mean nothing (see `crate::scope`, where every
    ///   caller's right on a shared domain comes from its instance role);
    /// * a principal that is not an existing, enabled account, so a typo is
    ///   reported instead of leaving a row waiting for someone to claim that
    ///   name later;
    /// * the domain's own owner, which is the one row that could only ever
    ///   *reduce* what its holder may do;
    /// * a membership with knowledge `none` and Reflection `default`, which
    ///   would grant nothing at all ([`RefusalKind::EmptyMembership`]).
    pub async fn upsert_domain_member(
        &self,
        domain: &str,
        principal: &str,
        level: MemberLevel,
        reflection: Option<ReflectionLevel>,
        added_by: &str,
    ) -> Result<()> {
        let domain = normalize_domain(domain)?;
        let principal = normalize_account_name(principal)?;
        let added_by = added_by.trim().to_string();
        if added_by.is_empty() {
            bail!("recording a membership needs an actor to record it as");
        }
        let now = chrono::Utc::now().to_rfc3339();
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("adding '{principal}' to domain '{domain}'"))?;
        let result = async {
            let Some(acl) = self.acl_of(&domain).await? else {
                return Err(refuse(
                    RefusalKind::NotPrivate,
                    format!(
                        "domain '{domain}' is not private, so it has no membership: \
                         make it private first"
                    ),
                ));
            };
            if acl.owner == principal {
                return Err(refuse(
                    RefusalKind::OwnerIsNotAMember,
                    format!(
                        "'{principal}' owns domain '{domain}': \
                         the owner already holds every level"
                    ),
                ));
            }
            self.require_live_user(&principal).await?;
            // `None` keeps what the row holds, read before the row is
            // replaced; a new member starts at `Default`.
            let reflection = match reflection {
                Some(r) => r,
                None => self.member_reflection_in_tx(&domain, &principal).await?,
            };
            if level == MemberLevel::None && reflection == ReflectionLevel::Default {
                return Err(refuse(
                    RefusalKind::EmptyMembership,
                    format!(
                        "a member with knowledge none needs a Reflection level (reader or \
                         contributor); remove '{principal}' from domain '{domain}' instead"
                    ),
                ));
            }
            self.conn
                .execute(
                    "DELETE FROM domain_member
                     WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(PRINCIPAL_USER.to_string()),
                        Value::Text(principal.clone()),
                    ],
                )
                .await
                .with_context(|| format!("adding '{principal}' to domain '{domain}'"))?;
            self.conn
                .execute(
                    "INSERT INTO domain_member
                         (domain, principal_kind, principal, level, reflection, added_by, added_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(PRINCIPAL_USER.to_string()),
                        Value::Text(principal.clone()),
                        Value::Text(level.as_str().to_string()),
                        reflection.to_db(),
                        Value::Text(added_by.clone()),
                        Value::Text(now.clone()),
                    ],
                )
                .await
                .with_context(|| format!("adding '{principal}' to domain '{domain}'"))?;
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Remove one membership. Returns whether a row was deleted, so a caller
    /// can tell "removed" from "was never a member" without a second read.
    pub async fn remove_domain_member(&self, domain: &str, principal: &str) -> Result<bool> {
        let domain = normalize_domain(domain)?;
        let principal = normalize_account_name(principal)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "DELETE FROM domain_member
                 WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                vec![
                    Value::Text(domain.clone()),
                    Value::Text(PRINCIPAL_USER.to_string()),
                    Value::Text(principal.clone()),
                ],
            )
            .await
            .with_context(|| format!("removing '{principal}' from domain '{domain}'"))?;
        Ok(changed > 0)
    }

    /// Set an account's Reflection level on every shared domain. `Default`
    /// clears it back to "as knowledge".
    pub async fn set_reflection_level(&self, name: &str, level: ReflectionLevel) -> Result<()> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "UPDATE users SET reflection = ?1 WHERE name = ?2",
                vec![level.to_db(), Value::Text(name.clone())],
            )
            .await
            .with_context(|| format!("setting the Reflection level of '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        Ok(())
    }

    /// One member's Reflection level on one private domain; `Default` when
    /// the account has no membership row there.
    pub async fn member_reflection(
        &self,
        domain: &str,
        principal: &str,
    ) -> Result<ReflectionLevel> {
        let domain = normalize_domain(domain)?;
        let principal = normalize_account_name(principal)?;
        let _guard = self.guard.lock().await;
        self.member_reflection_in_tx(&domain, &principal).await
    }

    /// [`AuthStore::member_reflection`] for an already folded domain and
    /// principal, without taking the guard: the caller holds it (or runs
    /// inside a transaction that does).
    async fn member_reflection_in_tx(
        &self,
        domain: &str,
        principal: &str,
    ) -> Result<ReflectionLevel> {
        let row = self
            .query_first(
                "SELECT reflection FROM domain_member
                 WHERE domain = ?1 AND principal_kind = ?2 AND principal = ?3",
                vec![
                    Value::Text(domain.to_string()),
                    Value::Text(PRINCIPAL_USER.to_string()),
                    Value::Text(principal.to_string()),
                ],
            )
            .await?;
        Ok(reflection_level_from_db(row.and_then(|r| cell_text(&r, 0))))
    }

    /// Give `account` the Reflection level `level` on `domain`, or change the
    /// level it holds there. A grant only raises what the account's engram
    /// access gives (see `crate::scope`); it works on a shared and on a
    /// private domain alike and opens no engram. The caller checks that the
    /// domain is kept as files: this store does not know how a domain is
    /// kept.
    ///
    /// Refused, inside the transaction that writes:
    ///
    /// * [`ReflectionLevel::Default`], which grants nothing; removing the
    ///   grant says that ([`RefusalKind::EmptyMembership`]);
    /// * an account that does not exist or is disabled, and the reserved
    ///   name [`crate::scope::OWNER_IDENTITY_NAME`], which is the machine
    ///   owner and curates every domain anyway ([`RefusalKind::NoSuchAccount`]).
    ///
    /// `added_by` is recorded as given (trimmed) and may not be empty.
    pub async fn set_reflection_grant(
        &self,
        domain: &str,
        account: &str,
        level: ReflectionLevel,
        added_by: &str,
    ) -> Result<()> {
        let domain = normalize_domain(domain)?;
        let account = normalize_account_name(account)?;
        let added_by = added_by.trim().to_string();
        if added_by.is_empty() {
            bail!("recording a Reflection collaborator needs an actor to record it as");
        }
        if level == ReflectionLevel::Default {
            return Err(refuse(
                RefusalKind::EmptyMembership,
                format!(
                    "a Reflection collaborator needs the level reader or contributor; \
                     to take '{account}' off domain '{domain}', use remove"
                ),
            ));
        }
        if account == crate::scope::OWNER_IDENTITY_NAME {
            return Err(refuse(
                RefusalKind::NoSuchAccount,
                format!("'{account}' is the reserved name of the machine owner, not an account"),
            ));
        }
        let now = chrono::Utc::now().to_rfc3339();
        let context = || format!("giving '{account}' the Reflection of domain '{domain}'");
        let _guard = self.guard.lock().await;
        self.begin_immediate().await.with_context(context)?;
        let result = async {
            self.require_live_user(&account).await?;
            self.conn
                .execute(
                    "DELETE FROM domain_reflection_grant WHERE domain = ?1 AND account = ?2",
                    vec![Value::Text(domain.clone()), Value::Text(account.clone())],
                )
                .await
                .with_context(context)?;
            self.conn
                .execute(
                    "INSERT INTO domain_reflection_grant (domain, account, level, added_by, added_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(account.clone()),
                        Value::Text(level.as_str().to_string()),
                        Value::Text(added_by.clone()),
                        Value::Text(now.clone()),
                    ],
                )
                .await
                .with_context(context)?;
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Take `account`'s Reflection grant on `domain` away. Answers whether
    /// there was one.
    pub async fn remove_reflection_grant(&self, domain: &str, account: &str) -> Result<bool> {
        let domain = normalize_domain(domain)?;
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(
                "DELETE FROM domain_reflection_grant WHERE domain = ?1 AND account = ?2",
                vec![Value::Text(domain.clone()), Value::Text(account.clone())],
            )
            .await
            .with_context(|| {
                format!("taking the Reflection of domain '{domain}' from '{account}'")
            })?;
        Ok(changed > 0)
    }

    /// Every Reflection collaborator of `domain`, by account name.
    pub async fn reflection_grants(&self, domain: &str) -> Result<Vec<ReflectionGrant>> {
        let domain = normalize_domain(domain)?;
        let context = || format!("listing the Reflection collaborators of domain '{domain}'");
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT account, level, added_by, added_at FROM domain_reflection_grant
                 WHERE domain = ?1 ORDER BY account",
                vec![Value::Text(domain.clone())],
            )
            .await
            .with_context(context)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.with_context(context)? {
            let Some(account) = cell_text(&row, 0) else {
                continue;
            };
            let level = reflection_level_from_db(cell_text(&row, 1));
            // An unreadable level grants nothing, so it is not listed as
            // a grant either.
            if level == ReflectionLevel::Default {
                continue;
            }
            out.push(ReflectionGrant {
                account,
                level,
                added_by: cell_text(&row, 2).unwrap_or_default(),
                added_at: cell_text(&row, 3).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// `account`'s Reflection grant on `domain`, or `None` when it holds
    /// none. A level this build cannot read is none: it fails closed.
    pub async fn reflection_grant(
        &self,
        domain: &str,
        account: &str,
    ) -> Result<Option<ReflectionLevel>> {
        let domain = normalize_domain(domain)?;
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        let row = self
            .query_first(
                "SELECT level FROM domain_reflection_grant WHERE domain = ?1 AND account = ?2",
                vec![Value::Text(domain), Value::Text(account)],
            )
            .await?;
        Ok(
            match reflection_level_from_db(row.and_then(|r| cell_text(&r, 0))) {
                ReflectionLevel::Default => None,
                level => Some(level),
            },
        )
    }

    /// Every private domain, by name. This is the whole input to the
    /// visibility filter: a domain absent from this list is visible to
    /// everyone who may reach the instance at all.
    pub async fn private_domains(&self) -> Result<Vec<DomainAcl>> {
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query("SELECT domain, owner FROM domain_acl ORDER BY domain", ())
            .await
            .context("listing the private domains")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.context("listing the private domains")? {
            // A row whose name cannot be read fails the whole call rather than
            // being skipped or defaulted. This list is the *input* to the
            // visibility filter, so dropping an entry (or defaulting it to
            // `""`, which is what it used to do) would leave a private domain
            // hidden from nobody. Skipping is the right answer one table over
            // in `memberships_of`, where a lost row only ever narrows what its
            // holder may reach; here it widens, so it must not be silent. The
            // column is `NOT NULL`, so this is a corrupt file, not a state the
            // schema permits.
            let Some(domain) = cell_text(&row, 0) else {
                bail!(
                    "the visibility record of a private domain is unreadable: \
                     refusing to answer rather than serving it to everyone"
                );
            };
            out.push(DomainAcl {
                domain,
                owner: cell_text(&row, 1).unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Every private domain `user` is a member of, with the level. Owned
    /// domains are not in here (an owner holds no membership row); the caller
    /// reads ownership from the acl rows it already has.
    pub async fn memberships_of(&self, user: &str) -> Result<Vec<(String, MemberLevel)>> {
        let user = normalize_account_name(user)?;
        let _guard = self.guard.lock().await;
        let mut rows = self
            .conn
            .query(
                "SELECT domain, level FROM domain_member
                 WHERE principal_kind = ?1 AND principal = ?2 ORDER BY domain",
                vec![
                    Value::Text(PRINCIPAL_USER.to_string()),
                    Value::Text(user.clone()),
                ],
            )
            .await
            .with_context(|| format!("listing the memberships of '{user}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("listing the memberships of '{user}'"))?
        {
            let Some(domain) = cell_text(&row, 0) else {
                continue;
            };
            out.push((
                domain,
                member_level_from_db(&cell_text(&row, 1).unwrap_or_default()),
            ));
        }
        Ok(out)
    }

    /// The domains `account` may write to, or `None` when it has no write
    /// list and writes wherever its role and memberships allow.
    ///
    /// An entry is a domain name or a name prefix ending in one `*`, stored
    /// as given (trimmed, never case folded, like a `domain_acl` row) and
    /// matched by [`crate::scope::write_list_matches`]. Sorted byte-wise.
    /// A list only lowers rights; [`crate::scope::DomainAccess`] applies it.
    pub async fn write_list(&self, account: &str) -> Result<Option<Vec<String>>> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        self.write_list_of(&account).await
    }

    /// Replace `account`'s write list with `patterns`: trimmed, empties
    /// dropped, duplicates kept once. An unknown account and a list with no
    /// entry are refused, because no rows already means "no list": an empty
    /// list cannot be stored, and removing the list is
    /// [`AuthStore::clear_write_list`]. The surface validates the entries; this
    /// stores what it is given.
    pub async fn set_write_list(&self, account: &str, patterns: &[String]) -> Result<()> {
        let account = normalize_account_name(account)?;
        let mut entries: Vec<String> = Vec::new();
        for pattern in patterns {
            let pattern = pattern.trim();
            if !pattern.is_empty() && !entries.iter().any(|e| e == pattern) {
                entries.push(pattern.to_string());
            }
        }
        if entries.is_empty() {
            bail!(
                "a write list needs at least one entry; to remove the list of '{account}', clear it"
            );
        }
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("setting the write list of '{account}'"))?;
        let result = async {
            self.require_user_row(&account).await?;
            self.delete_write_list_of(&account).await?;
            for pattern in &entries {
                self.conn
                    .execute(
                        "INSERT INTO account_write_domain (account, pattern) VALUES (?1, ?2)",
                        vec![Value::Text(account.clone()), Value::Text(pattern.clone())],
                    )
                    .await
                    .with_context(|| format!("setting the write list of '{account}'"))?;
            }
            Ok(())
        }
        .await;
        self.finish(result).await
    }

    /// Remove `account`'s write list, so it writes wherever its role and
    /// memberships allow. An unknown account is refused; an account with no
    /// list is fine.
    pub async fn clear_write_list(&self, account: &str) -> Result<()> {
        let account = normalize_account_name(account)?;
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("clearing the write list of '{account}'"))?;
        let result = async {
            self.require_user_row(&account).await?;
            self.delete_write_list_of(&account).await
        }
        .await;
        self.finish(result).await
    }

    /// [`AuthStore::write_list`] without the lock or the folding.
    async fn write_list_of(&self, account: &str) -> Result<Option<Vec<String>>> {
        let mut rows = self
            .conn
            .query(
                "SELECT pattern FROM account_write_domain WHERE account = ?1 ORDER BY pattern",
                vec![Value::Text(account.to_string())],
            )
            .await
            .with_context(|| format!("reading the write list of '{account}'"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("reading the write list of '{account}'"))?
        {
            if let Some(pattern) = cell_text(&row, 0) {
                out.push(pattern);
            }
        }
        Ok((!out.is_empty()).then_some(out))
    }

    /// Ends every draft grant `name` holds as grantee, so an account made later
    /// under the name opens nobody's draft. What `name` gave to others ends with
    /// each of its drafts ([`AuthStore::end_overlay_grants`]).
    async fn delete_overlay_grants_held_by(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM overlay_grant WHERE grantee = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing the draft grants of user '{name}'"))?;
        Ok(())
    }

    /// Delete every write-list row of one account. Inside the caller's
    /// transaction; no lock taken.
    async fn delete_write_list_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM account_write_domain WHERE account = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing the write list of user '{name}'"))?;
        Ok(())
    }

    /// `no such user` unless `name` (already folded) has a row. Inside the
    /// caller's transaction, so the check and the write see one table.
    async fn require_user_row(&self, name: &str) -> Result<()> {
        if self
            .query_first(
                "SELECT 1 FROM users WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await?
            .is_none()
        {
            bail!("no such user: '{name}'");
        }
        Ok(())
    }

    /// Un-name `name` as the owner of every private domain it owns, leaving
    /// each row owned by nobody.
    ///
    /// The other half of the removal sweep, and the half that closes the
    /// resurrection hazard on this table: `domain_acl` carries no foreign key,
    /// so an owner row that kept a freed login name would hand ownership - read,
    /// write, membership management and the power to make the domain shared
    /// again - to whoever is next added under that name. That is the same
    /// hazard `a_readded_name_does_not_inherit_the_old_holders_session` pins for
    /// sessions and [`AuthStore::mcp_token_user`] for tokens, one table over,
    /// and here it would hand a private domain to a stranger.
    ///
    /// The empty string is the "owned by nobody" marker because
    /// [`normalize_account_name`] can never produce it, so no live account can ever
    /// match it. The domain stays private and its members keep their levels;
    /// what it loses is an owner, which leaves it administered by instance
    /// admins alone until one runs [`AuthStore::transfer_domain`]. Choosing a
    /// successor is deliberately not done here - that is a product decision,
    /// and every automatic answer (the removing admin, the senior manager)
    /// hands somebody a domain nobody gave them.
    async fn disown_domains_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE domain_acl SET owner = '' WHERE owner = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("releasing the domains owned by user '{name}'"))?;
        Ok(())
    }

    /// Fail unless `name` is an existing account that is not disabled. Called
    /// inside a transaction, so what it checked is what the write beside it
    /// sees.
    ///
    /// A disabled account is refused rather than accepted: it cannot sign in,
    /// so granting it access would be a row nobody can use today and a
    /// surprise the day the account is re-enabled.
    async fn require_live_user(&self, name: &str) -> Result<()> {
        let row = self
            .query_first(
                "SELECT disabled FROM users WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await?;
        // Both arms carry the SAME kind. The two states are told apart in the
        // message, for the operator-facing surfaces that may see it, and never
        // by the type - so a surface that collapses them cannot accidentally
        // grow a branch that does not.
        match row {
            None => Err(refuse(
                RefusalKind::NoSuchAccount,
                format!("no such user: '{name}'"),
            )),
            Some(row) if matches!(row.get_value(0), Ok(Value::Integer(i)) if i != 0) => {
                Err(refuse(
                    RefusalKind::NoSuchAccount,
                    format!("user '{name}' is disabled: enable the account first"),
                ))
            }
            Some(_) => Ok(()),
        }
    }

    /// Drop every membership row of one domain. Callers hold the lock and are
    /// inside a transaction.
    ///
    /// The one statement here that deliberately carries no `principal_kind`
    /// filter, where every sibling has one: a domain that is no longer private
    /// has no membership of any kind, so this must sweep a future group row
    /// too. Not an omission - do not "fix" it.
    async fn delete_members_of_domain(&self, domain: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM domain_member WHERE domain = ?1",
                vec![Value::Text(domain.to_string())],
            )
            .await
            .with_context(|| format!("removing the members of domain '{domain}'"))?;
        Ok(())
    }

    /// Drop every membership row `name` holds, wherever it holds one.
    ///
    /// Called from both removal paths for the reason
    /// `a_readded_name_does_not_inherit_the_old_holders_session` pins for
    /// sessions: `domain_member` carries no foreign key, so a row left behind
    /// would be inherited by the next account to claim the same login name -
    /// here that would hand a stranger read access to a private domain. The
    /// belt to [`AuthStore::mcp_token_user`]'s suspenders, one table over.
    ///
    /// Deliberately not called by [`AuthStore::set_disabled`]: disabling is
    /// reversible and `crate::scope` already refuses a disabled account at
    /// resolve time, so re-enabling must hand the memberships back rather than
    /// force every invitation to be issued again.
    ///
    /// Ownership is handled beside this, by [`AuthStore::disown_domains_of`].
    async fn delete_memberships_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM domain_member WHERE principal_kind = ?1 AND principal = ?2",
                vec![
                    Value::Text(PRINCIPAL_USER.to_string()),
                    Value::Text(name.to_string()),
                ],
            )
            .await
            .with_context(|| format!("removing the memberships of user '{name}'"))?;
        Ok(())
    }

    /// Drop every Reflection grant `name` holds, for the reason
    /// [`AuthStore::delete_memberships_of`] drops its membership rows: the
    /// next account to take the name must not inherit them.
    async fn delete_reflection_grants_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM domain_reflection_grant WHERE account = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing the Reflection grants of user '{name}'"))?;
        Ok(())
    }

    /// The two conflict checks and the insert that make one link, with no
    /// lock taken and no transaction opened.
    ///
    /// Both public linking paths call this after taking the guard and opening
    /// their own transaction, because neither the guard (a `tokio` mutex) nor
    /// a turso transaction is reentrant: a method calling the other public
    /// method would deadlock on the first and be refused on the second.
    async fn insert_link(
        &self,
        issuer: &str,
        subject: &str,
        user: &str,
        linked_by: &str,
    ) -> Result<()> {
        let pair = vec![
            Value::Text(issuer.to_string()),
            Value::Text(subject.to_string()),
        ];
        if let Some(row) = self
            .query_first(
                "SELECT user FROM identity_link WHERE issuer = ?1 AND subject = ?2",
                pair.clone(),
            )
            .await?
        {
            let holder = cell_text(&row, 0).unwrap_or_default();
            return Err(refuse(
                RefusalKind::IdentityAlreadyLinked,
                format!(
                    "this identity is already linked to account '{holder}': an admin can move it"
                ),
            ));
        }
        if self
            .query_first(
                "SELECT subject FROM identity_link WHERE issuer = ?1 AND user = ?2",
                vec![
                    Value::Text(issuer.to_string()),
                    Value::Text(user.to_string()),
                ],
            )
            .await?
            .is_some()
        {
            return Err(refuse(
                RefusalKind::IssuerAlreadyHeld,
                format!(
                    "account '{user}' already holds an identity at this provider: unlink that \
                     one first"
                ),
            ));
        }
        self.conn
            .execute(
                "INSERT INTO identity_link (issuer, subject, user, linked_at, linked_by)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                vec![
                    Value::Text(issuer.to_string()),
                    Value::Text(subject.to_string()),
                    Value::Text(user.to_string()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                    Value::Text(linked_by.to_string()),
                ],
            )
            .await
            .with_context(|| format!("linking an identity to user '{user}'"))?;
        Ok(())
    }

    /// The first free account name from `base`, `base-2`, `base-3` and so on.
    /// Called inside a transaction, so the name it found is still free when
    /// the insert beside it runs.
    async fn free_account_name(&self, base: &str) -> Result<String> {
        for attempt in 1..=MAX_NAME_ATTEMPTS {
            let candidate = if attempt == 1 {
                base.to_string()
            } else {
                format!("{base}-{attempt}")
            };
            if self
                .query_first(
                    "SELECT 1 FROM users WHERE name = ?1",
                    vec![Value::Text(candidate.clone())],
                )
                .await?
                .is_none()
            {
                return Ok(candidate);
            }
        }
        bail!("refusing to provision an account: every name from '{base}' on is taken")
    }

    /// Whether `user` holds a link at `issuer`. Callers hold the lock and are
    /// inside a transaction.
    async fn link_at(&self, issuer: &str, user: &str) -> Result<bool> {
        Ok(self
            .query_first(
                "SELECT 1 FROM identity_link WHERE issuer = ?1 AND user = ?2",
                vec![
                    Value::Text(issuer.to_string()),
                    Value::Text(user.to_string()),
                ],
            )
            .await?
            .is_some())
    }

    /// How many identities `user` holds, across every issuer. Callers hold the
    /// lock and are inside a transaction.
    async fn link_count(&self, user: &str) -> Result<i64> {
        let row = self
            .query_first(
                "SELECT COUNT(*) FROM identity_link WHERE user = ?1",
                vec![Value::Text(user.to_string())],
            )
            .await?;
        Ok(match row.as_ref().map(|row| row.get_value(0)) {
            Some(Ok(Value::Integer(count))) => count,
            _ => 0,
        })
    }

    /// Whether the account row for `name` carries a password hash. Callers
    /// hold the lock; the two transactional callers are inside one.
    ///
    /// The hash itself is never read out: what comes back is the one bit the
    /// question needs, so no caller can grow a habit of holding one.
    async fn password_present(&self, name: &str) -> Result<bool> {
        let row = self
            .query_first(
                "SELECT pass_hash IS NOT NULL AND pass_hash <> '' FROM users WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await?;
        Ok(matches!(
            row.as_ref().map(|row| row.get_value(0)),
            Some(Ok(Value::Integer(present))) if present != 0
        ))
    }

    /// Drop the GitHub link of one account, inside the removal transaction.
    async fn delete_github_link_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM github_link WHERE account = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing the GitHub link of user '{name}'"))?;
        Ok(())
    }

    /// Drop every identity linked to one account. Called inside the removal
    /// transaction: a link that outlived its account would hand the next
    /// account to claim the name somebody else's sign-on.
    async fn delete_identity_links_of(&self, name: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM identity_link WHERE user = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .with_context(|| format!("removing the identity links of user '{name}'"))?;
        Ok(())
    }

    /// Run a single-column update against one account, failing when the account
    /// does not exist, for a statement carrying the [`NOT_LAST_ADMIN`] guard:
    /// zero rows changed then has a second possible meaning, that the edit was
    /// refused because it would have left no enabled admin. `verb` names the
    /// refused operation in that message.
    ///
    /// The follow-up existence probe only picks between the two messages -
    /// nothing was written either way - so it does not need to share the
    /// statement's transaction.
    async fn update_guarded(&self, sql: &str, name: &str, value: Value, verb: &str) -> Result<()> {
        let name = normalize_account_name(name)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(sql, vec![Value::Text(name.clone()), value])
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        if changed == 0 {
            let exists = self
                .query_first(
                    "SELECT 1 FROM users WHERE name = ?1",
                    vec![Value::Text(name.clone())],
                )
                .await?;
            if exists.is_some() {
                return Err(last_admin_error(verb, &name));
            }
            bail!("no such user: '{name}'");
        }
        Ok(())
    }

    /// Open a write transaction that takes the write lock immediately rather
    /// than on first write. Deferred would let two processes both start, both
    /// read, and only then discover they conflict; immediate makes the two
    /// multi-statement operations here serialize cleanly, which is the whole
    /// point of using one.
    async fn begin_immediate(&self) -> Result<()> {
        self.conn
            .execute("BEGIN IMMEDIATE", ())
            .await
            .context("opening an auth database transaction")?;
        Ok(())
    }

    /// Commit when the body succeeded, roll back when it did not. The rollback
    /// is best-effort: the body's error is what the caller needs to see, and
    /// an abandoned transaction is released when the connection drops anyway.
    async fn finish<T>(&self, result: Result<T>) -> Result<T> {
        match result {
            Ok(value) => {
                self.conn
                    .execute("COMMIT", ())
                    .await
                    .context("committing an auth database transaction")?;
                Ok(value)
            }
            Err(e) => {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                Err(e)
            }
        }
    }

    /// First row of a query, draining the rest so the statement finishes
    /// instead of rolling back when the cursor drops.
    async fn query_first(&self, sql: &str, params: Vec<Value>) -> Result<Option<Row>> {
        let mut rows = self
            .conn
            .query(sql, params)
            .await
            .context("querying the auth database")?;
        let first = rows.next().await.context("querying the auth database")?;
        while rows
            .next()
            .await
            .context("querying the auth database")?
            .is_some()
        {}
        Ok(first)
    }
}

/// Add a column to an existing table when it is missing. The auth database has
/// no schema-version counter - its `SCHEMA` is idempotent DDL - so a new column
/// follows the same contract: run the ALTER, and treat "the column is already
/// there" as success. Any other failure is real and propagates.
async fn ensure_column(conn: &Connection, table: &str, column_def: &str) -> Result<()> {
    match conn
        .execute(&format!("ALTER TABLE {table} ADD COLUMN {column_def}"), ())
        .await
    {
        Ok(_) => Ok(()),
        Err(e)
            if e.to_string()
                .to_ascii_lowercase()
                .contains("duplicate column") =>
        {
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("adding {table}.{column_def}")),
    }
}

/// Decode the [`USER_COLUMNS`] prefix of a row.
fn user_from_row(row: &Row) -> User {
    User {
        name: cell_text(row, 0).unwrap_or_default(),
        display: cell_text(row, 1).unwrap_or_default(),
        email: cell_text(row, 2),
        role: role_from_db(&cell_text(row, 3).unwrap_or_default()),
        disabled: matches!(row.get_value(4), Ok(Value::Integer(i)) if i != 0),
        last_seen: cell_text(row, 5),
        reflection: reflection_level_from_db(cell_text(row, 6)),
    }
}

/// Decode one `overlay_grant` row, in the column order every query above
/// selects. One decoder rather than four, so a column added later cannot be
/// read in two different orders.
fn grant_from_row(row: &Row) -> OverlayGrant {
    OverlayGrant {
        id: match row.get_value(0) {
            Ok(Value::Integer(id)) => id,
            _ => 0,
        },
        domain: cell_text(row, 1).unwrap_or_default(),
        path: cell_text(row, 2).unwrap_or_default(),
        owner: cell_text(row, 3).unwrap_or_default(),
        grantee: cell_text(row, 4),
        created_at: cell_text(row, 5).unwrap_or_default(),
        expires_at: cell_text(row, 6),
        revoked_at: cell_text(row, 7),
    }
}

fn cell_text(row: &Row, idx: usize) -> Option<String> {
    match row.get_value(idx) {
        Ok(Value::Text(s)) => Some(s),
        _ => None,
    }
}

/// `N` bytes from the OS CSPRNG, lowercase hex. The single entropy source
/// behind every random value this file mints, so an audit of "where does the
/// randomness come from" reads one function rather than one per family.
fn random_hex_bytes<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG is available");
    crystalline_index::hex_lower(&bytes)
}

/// 32 bytes, 64 hex characters: the random half of every credential here - a
/// session token, its CSRF token, an MCP token, and an OAuth access and
/// refresh token.
fn random_hex() -> String {
    random_hex_bytes::<32>()
}

/// 16 bytes, 32 hex characters: the random half of an OAuth client id. Half
/// the width of [`random_hex`] on purpose - a client id is a public identifier
/// that authorizes nothing by itself, and 128 random bits is already far past
/// guessing.
fn random_id_hex() -> String {
    random_hex_bytes::<16>()
}

/// A resource identifier as the audience check compares it: trimmed, with at
/// most one trailing slash taken off.
///
/// Exactly that tolerance and nothing more. Nothing is lowercased and no other
/// canonicalization happens, because an origin that differs in any other way
/// is a different origin, and an audience check that "helpfully" folded
/// spellings together would be the hole it exists to close. The trailing slash
/// is the one difference clients actually produce, since a person typing the
/// server address into a client adds or omits it without meaning anything by
/// it.
///
/// `pub(crate)` so the request-origin side compares the same spelling this one
/// stores; a second implementation of this rule is a bug waiting to happen.
#[doc(hidden)]
pub fn normalize_resource(resource: &str) -> String {
    let trimmed = resource.trim();
    trimmed.strip_suffix('/').unwrap_or(trimmed).to_string()
}

/// The host (with its port, when the uri names one) a client is redirected
/// back to, for the consent screen and the grant list.
///
/// Display only. Whether a redirect uri is acceptable, and whether the one
/// presented at the token endpoint matches the registration, are decisions
/// made against the registered uris themselves - never against this string.
///
/// A uri that names no host - one that does not parse, or a `javascript:` or
/// `data:` uri, which parse and carry none - reads as [`NO_REDIRECT_HOST`]
/// rather than being echoed. Registration validation should make both
/// unreachable; this is the last line before attacker-chosen text lands in a
/// field a person reads as the address they are being asked to recognize.
#[doc(hidden)]
pub fn redirect_host(uri: &str) -> String {
    let Ok(parsed) = url::Url::parse(uri) else {
        return NO_REDIRECT_HOST.to_string();
    };
    match (parsed.host_str(), parsed.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        (None, _) => NO_REDIRECT_HOST.to_string(),
    }
}

/// Decode the JSON array a registration's redirect uris are stored as.
///
/// A missing or unreadable value reads as no uris at all, which fails closed:
/// a registration with nothing to match against can complete no flow, where a
/// tolerant fallback would have to invent a uri to redirect to.
fn decode_redirect_uris(encoded: Option<&str>) -> Vec<String> {
    encoded
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

/// A unix timestamp column as the RFC 3339 string a person reads. The expiry
/// columns are integers so every comparison happens in SQL; this is the one
/// place that turns one back into text, on the way out to a management screen.
fn rfc3339_from_unix(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|when| when.to_rfc3339())
        .unwrap_or_default()
}

/// What is stored for a session token. The token itself is never written, so a
/// copy of the database yields nothing replayable.
fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    crystalline_index::hex_lower(&hasher.finalize())
}

/// Hash a password with argon2id at the crate's recommended defaults. Argon2
/// is deliberately expensive in both time and memory, so it runs on the
/// blocking pool: a login must not stall the runtime worker that other
/// requests are sharing. The salt is generated internally, under argon2's
/// `getrandom` feature, rather than passed in: 0.6's `hash_password` no
/// longer takes one.
async fn hash_password(password: &str) -> Result<String> {
    let password = password.to_string();
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(|e| anyhow!("hashing a password failed: {e}"))
    })
    .await
    .context("the password hashing task failed")?
}

#[cfg(any(test, feature = "testing"))]
tokio::task_local! {
    /// Counts the argon2 verifications run inside one test's task, real or
    /// dummy.
    ///
    /// The point of the dummy verification is that a login attempt costs the
    /// same wherever it lands, and a code path that merely looks balanced is
    /// not evidence of that. This lets a test assert the cost itself: exactly
    /// one verification per attempt, on every path. Test-only; nothing in a
    /// served binary touches it.
    ///
    /// Task-local rather than a process-wide atomic, because `cargo test` runs
    /// the tests of one binary as threads in a single process: a global counter
    /// would also count whatever a sibling test was verifying at that instant,
    /// and the "exactly one" assertion would be a race rather than a property.
    /// Every verification a login runs happens on the task that asked for it,
    /// so a scope around a test body counts exactly that test's own work. A
    /// verification outside any scope (every other test, and the served binary)
    /// simply counts nowhere.
    #[doc(hidden)]
    pub static VERIFICATIONS: std::cell::Cell<u64>;
}

/// Verify `password` against a hash no account has, and throw the answer away.
///
/// The point is the time it takes, not the result.
/// [`AuthStore::verify_password`] returns before any hashing when there is no
/// hash to check against - an unknown name, a disabled account, an account
/// provisioned without a password - so a caller that answers as soon as it
/// hears `None` answers a miss faster than a wrong password. Running this on
/// those paths costs the same argon2id work a real check costs, which is what
/// removes the difference. See `rest::auth::check_password`, the caller that
/// owes it.
///
/// The hash is derived once per process from random bytes, so it is a real
/// hash at the crate's current cost parameters (a frozen constant here would
/// drift from them) and no password can match it.
#[doc(hidden)]
pub async fn dummy_verify(password: &str) -> Result<bool> {
    static DUMMY: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    let hash = DUMMY
        .get_or_try_init(|| async { hash_password(&random_hex()).await })
        .await?;
    verify_hash(hash.clone(), password.to_string()).await
}

/// Verify a password against a stored PHC string, on the blocking pool for the
/// same reason as [`hash_password`]. A hash this cannot parse verifies as
/// false rather than erroring: a corrupt row must fail closed.
async fn verify_hash(hash: String, password: String) -> Result<bool> {
    #[cfg(any(test, feature = "testing"))]
    let _ = VERIFICATIONS.try_with(|count| count.set(count.get() + 1));
    tokio::task::spawn_blocking(move || match PasswordHash::new(&hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    })
    .await
    .context("the password verification task failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether [`open_database`] is expected to reach multiprocess WAL on the
    /// platform these tests are running on, rather than falling back to a
    /// legacy open. Observable as the `web-auth.db-tshm` coordination file
    /// beside the database, which only the multiprocess open creates.
    ///
    /// Stated once here, with its two upstream conditions, so no assertion has
    /// to carry a `cfg` of its own:
    ///
    /// * turso_core's `host_shared_wal` cfg, which its `build.rs` sets to
    ///   `all(any(unix, target_os = "windows"), target_pointer_width = "64")`.
    ///   Where it is off the flag is a documented no-op and legacy behavior
    ///   stays, so a 32-bit target never gets the coordination file.
    /// * an IO backend whose `supports_shared_wal_coordination` is true
    ///   (turso_core 0.7.2 and 0.8.1 `io/mod.rs`, where the trait default is `false`).
    ///   The unix and io_uring backends override it to `true`; the default
    ///   Windows backend, `WindowsIO`, does not, and only `WindowsIOCP`,
    ///   compiled only under the off-by-default `experimental_win_iocp` cargo
    ///   feature, does. So a Windows open takes the fallback.
    ///
    /// The day either changes upstream, whatever reads this goes red rather
    /// than quietly stale, which is the point of asserting the mode at all.
    const MULTIPROCESS_WAL_EXPECTED: bool = cfg!(unix) && cfg!(target_pointer_width = "64");

    async fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        (dir, store)
    }

    /// A GitHub delivery id is new once: the second time within a day it is
    /// a replay.
    #[tokio::test]
    async fn a_delivery_is_remembered_once() {
        let (_dir, store) = store().await;
        assert!(store.remember_delivery("d-1").await.unwrap());
        assert!(!store.remember_delivery("d-1").await.unwrap());
        assert!(store.remember_delivery("d-2").await.unwrap());
    }

    /// A delivery older than a day is pruned, so its id counts as new again;
    /// one younger than a day stays.
    #[tokio::test]
    async fn a_delivery_older_than_a_day_is_forgotten() {
        let (_dir, store) = store().await;
        let now = chrono::Utc::now().timestamp();
        assert!(
            store
                .remember_delivery_at("old", now - 86_401)
                .await
                .unwrap()
        );
        assert!(
            store
                .remember_delivery_at("young", now - 86_399)
                .await
                .unwrap()
        );
        assert!(store.remember_delivery_at("old", now).await.unwrap());
        assert!(!store.remember_delivery_at("young", now).await.unwrap());
    }

    /// A delivery whose handling failed is forgotten, so GitHub's redelivery
    /// is handled.
    #[tokio::test]
    async fn a_forgotten_delivery_is_new_again() {
        let (_dir, store) = store().await;
        assert!(store.remember_delivery("d-1").await.unwrap());
        store.forget_delivery("d-1").await.unwrap();
        assert!(store.remember_delivery("d-1").await.unwrap());
    }

    #[tokio::test]
    async fn password_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        assert!(
            store
                .verify_password("ada", "correct horse")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .verify_password("ada", "wrong")
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The brief mandates argon2id specifically, and `Argon2::default()` is
    /// what supplies it, so pin the algorithm the PHC string records. The cost
    /// parameters are deliberately not pinned: those are meant to track the
    /// crate's recommendation, not a number frozen here.
    #[tokio::test]
    async fn passwords_are_stored_as_argon2id_phc_strings() {
        let hash = hash_password("hunter2").await.unwrap();
        assert!(
            hash.starts_with("$argon2id$"),
            "expected an argon2id PHC string, got {hash}"
        );
        assert!(
            verify_hash(hash.clone(), "hunter2".to_string())
                .await
                .unwrap()
        );
        assert!(!verify_hash(hash, "other".to_string()).await.unwrap());
    }

    /// A hash written by argon2 0.5.3 still verifies after the 0.6 bump. Every
    /// account provisioned before the upgrade carries one of these, so this is
    /// the test that says an upgrade is not a mass password reset. Captured
    /// verbatim from argon2 0.5.3 with `Argon2::default()` on 2026-09-14; the
    /// password it was written for is beside it, because a hash without its
    /// password pins nothing.
    #[tokio::test]
    async fn a_hash_written_under_argon2_0_5_still_verifies() {
        const PASSWORD: &str = "correct horse battery staple";
        const HASH_0_5: &str = "$argon2id$v=19$m=19456,t=2,p=1$lU1KHTS8csu8CtBAN/g/Zw$gq+EDJ0qxuAHNNk1oxZGDP88YjooUWCCp14VWhB+b30";

        assert!(
            verify_hash(HASH_0_5.to_string(), PASSWORD.to_string())
                .await
                .unwrap(),
            "the stored parameters travel in the PHC string, so the hash verifies whatever this \
             crate's current defaults are"
        );
        assert!(
            !verify_hash(HASH_0_5.to_string(), "wrong".to_string())
                .await
                .unwrap(),
            "and it still refuses the wrong password"
        );
    }

    #[tokio::test]
    async fn the_same_password_hashes_differently_each_time() {
        let a = hash_password("pw").await.unwrap();
        let b = hash_password("pw").await.unwrap();
        assert_ne!(a, b, "every hash carries its own random salt");
    }

    #[tokio::test]
    async fn a_corrupt_hash_fails_closed() {
        assert!(
            !verify_hash("not a phc string".to_string(), "pw".to_string())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn unknown_user_never_verifies() {
        let (_dir, store) = store().await;
        assert!(
            store
                .verify_password("nobody", "pw")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn disabled_user_never_verifies() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        store.set_disabled("ada", true).await.unwrap();
        assert!(store.verify_password("ada", "pw").await.unwrap().is_none());
        store.set_disabled("ada", false).await.unwrap();
        assert!(store.verify_password("ada", "pw").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn session_create_lookup_and_expiry() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", Some("ada@example.com"), Role::Editor, "pw")
            .await
            .unwrap();

        let live = store.create_session("ada", 3600).await.unwrap();
        assert_eq!(live.token.len(), 64);
        assert_eq!(live.csrf.len(), 64);
        assert_ne!(live.token, live.csrf);
        let (user, csrf) = store.session_user(&live.token).await.unwrap().unwrap();
        assert_eq!(user.name, "ada");
        assert_eq!(user.email.as_deref(), Some("ada@example.com"));
        assert_eq!(user.role, Role::Editor);
        assert_eq!(csrf, live.csrf);

        let expired = store.create_session("ada", -1).await.unwrap();
        assert!(store.session_user(&expired.token).await.unwrap().is_none());

        assert!(store.session_user("not-a-token").await.unwrap().is_none());
        // The live session survived the expired one's prune.
        assert!(store.session_user(&live.token).await.unwrap().is_some());
    }

    /// The quiet lookup answers what `session_user` answers and writes
    /// nothing. Catches a lookup that stamped `last_seen_at` (every open tab
    /// would read as a fresh sighting) or pruned (a write per check), and
    /// one that let an expired session or a disabled account through. The
    /// account is disabled under its session row here: `set_disabled`
    /// deletes the sessions with it, so only a row an older build left
    /// behind reaches the filter, and the filter is what keeps it out.
    #[tokio::test]
    async fn the_quiet_session_lookup_reads_without_writing() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let live = store.create_session("ada", 3600).await.unwrap();
        let expired = store.create_session("ada", -1).await.unwrap();
        let seen = store.user("ada").await.unwrap().unwrap().last_seen;
        let user = store
            .session_user_quiet(&live.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((user.name.as_str(), user.role), ("ada", Role::Editor));
        assert!(
            store
                .session_user_quiet(&expired.token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .session_user_quiet("not-a-token")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.user("ada").await.unwrap().unwrap().last_seen,
            seen,
            "not stamped"
        );
        assert_eq!(
            store.session_count().await.unwrap(),
            2,
            "the expired row was not pruned"
        );
        store.set_role("ada", Role::Viewer).await.unwrap();
        let user = store
            .session_user_quiet(&live.token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.role, Role::Viewer, "the stored role, read now");
        disable_keeping_sessions(&store, "ada").await;
        assert!(
            store
                .session_user_quiet(&live.token)
                .await
                .unwrap()
                .is_none(),
            "a disabled account's surviving session row answers none"
        );
    }

    /// Flag `name` disabled and leave its session rows alone, the state a
    /// file written before disabling revoked sessions can hold.
    async fn disable_keeping_sessions(store: &AuthStore, name: &str) {
        let _guard = store.guard.lock().await;
        store
            .conn
            .execute(
                "UPDATE users SET disabled = 1 WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await
            .unwrap();
    }

    /// `ensure_session` issues at most one session per account: the first call
    /// creates, every later one reuses, and only an account with nothing live
    /// gets a second row. This is what keeps `/auth/me` from adding a session
    /// per probe for a trusted-header client that keeps no cookie.
    #[tokio::test]
    async fn ensure_session_reuses_a_live_session_rather_than_adding_one() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        assert_eq!(store.session_count().await.unwrap(), 0);
        assert!(store.newest_session_csrf("ada").await.unwrap().is_none());

        let SessionMint::Created(first) = store.ensure_session("ada", 3600).await.unwrap() else {
            panic!("the first call has nothing to reuse, so it creates");
        };
        assert_eq!(store.session_count().await.unwrap(), 1);

        // Ten more probes, all reusing: the row count does not move and the
        // token does not change, which is what keeps a second tab working.
        for _ in 0..10 {
            let mint = store.ensure_session("AdA", 3600).await.unwrap();
            assert!(
                matches!(mint, SessionMint::Reused { .. }),
                "a live session must be reused, not duplicated"
            );
            assert_eq!(mint.csrf(), first.csrf);
        }
        assert_eq!(store.session_count().await.unwrap(), 1);
        assert_eq!(
            store.newest_session_csrf("ada").await.unwrap().as_deref(),
            Some(first.csrf.as_str())
        );
        // The created session is still the one the cookie resolves to.
        let (_, csrf) = store.session_user(&first.token).await.unwrap().unwrap();
        assert_eq!(csrf, first.csrf);

        // An expired session is not live, so the next probe issues a fresh one
        // and takes the dead row with it. `session_user` is the only other
        // pruner and a cookieless probe never reaches it, so without this an
        // account whose session lapsed would leave a row behind every time.
        store.delete_session(&first.token).await.unwrap();
        store.create_session("ada", -1).await.unwrap();
        assert_eq!(store.session_count().await.unwrap(), 1, "the expired row");
        assert!(store.newest_session_csrf("ada").await.unwrap().is_none());
        let SessionMint::Created(second) = store.ensure_session("ada", 3600).await.unwrap() else {
            panic!("nothing live is left to reuse, so this creates");
        };
        assert_ne!(second.csrf, first.csrf);
        assert_eq!(
            store.session_count().await.unwrap(),
            1,
            "the expired row was pruned rather than left beside the new one"
        );

        // Another account's live session is never handed over.
        store
            .add_user("bob", "Bob", None, Role::Editor, "pw")
            .await
            .unwrap();
        assert!(store.newest_session_csrf("bob").await.unwrap().is_none());
        let SessionMint::Created(bobs) = store.ensure_session("bob", 3600).await.unwrap() else {
            panic!("bob holds nothing, so this creates");
        };
        assert_ne!(bobs.csrf, second.csrf);

        // An account nobody created has no session to ensure.
        assert!(store.ensure_session("ghost", 3600).await.is_err());
    }

    /// Concurrent probes, which is the two-tabs-at-once case: the check and the
    /// insert are one transaction, so exactly one session is created however
    /// many arrive together, and every caller is handed the same token. Without
    /// the transaction each tab would get its own token and only the one whose
    /// `Set-Cookie` landed last would be able to write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_probes_settle_on_one_session() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let store = std::sync::Arc::new(store);

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store.ensure_session("ada", 3600).await.unwrap()
            }));
        }
        let mints: Vec<SessionMint> = futures::future::join_all(tasks)
            .await
            .into_iter()
            .map(|t| t.unwrap())
            .collect();

        assert_eq!(
            store.session_count().await.unwrap(),
            1,
            "one session however many probes raced for it"
        );
        let created = mints
            .iter()
            .filter(|m| matches!(m, SessionMint::Created(_)))
            .count();
        assert_eq!(created, 1, "exactly one caller created it");
        let token = mints[0].csrf();
        for mint in &mints {
            assert_eq!(mint.csrf(), token, "every tab was handed the same token");
        }
    }

    #[tokio::test]
    async fn a_disabled_user_loses_a_live_session() {
        let (_dir, store) = store().await;
        // Editor rather than admin: the role is incidental here, and the last
        // enabled admin cannot be disabled (see [`NOT_LAST_ADMIN`]).
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let s = store.create_session("ada", 3600).await.unwrap();
        store.set_disabled("ada", true).await.unwrap();
        assert!(store.session_user(&s.token).await.unwrap().is_none());
    }

    /// Disabling is a revocation, not a flag over the session rows: re-enabling
    /// the account must not hand back the cookies it held before. Otherwise
    /// disabling a compromised account and enabling it again once the password
    /// was changed would restore the intruder's session.
    #[tokio::test]
    async fn re_enabling_does_not_resurrect_the_sessions_disabling_took() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let s = store.create_session("ada", 3600).await.unwrap();
        store.set_disabled("ada", true).await.unwrap();
        store.set_disabled("ada", false).await.unwrap();
        assert!(
            store.session_user(&s.token).await.unwrap().is_none(),
            "the session was deleted, not merely hidden while the flag was set"
        );
        // The account itself is back, and a fresh session works.
        let fresh = store.create_session("ada", 3600).await.unwrap();
        assert!(store.session_user(&fresh.token).await.unwrap().is_some());
    }

    /// A password reset evicts whoever was signed in under the old one: a
    /// session never presents a password again, so without this the holder of a
    /// cookie minted before the reset keeps the account.
    #[tokio::test]
    async fn changing_a_password_revokes_the_sessions_it_issued() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let before = store.create_session("ada", 3600).await.unwrap();
        store.set_password("ada", "new pw").await.unwrap();
        assert!(
            store.session_user(&before.token).await.unwrap().is_none(),
            "the cookie from before the reset is dead"
        );
        assert!(
            store
                .verify_password("ada", "new pw")
                .await
                .unwrap()
                .is_some(),
            "and the new password logs in"
        );
        let after = store.create_session("ada", 3600).await.unwrap();
        assert!(store.session_user(&after.token).await.unwrap().is_some());
    }

    /// A refused edit revokes nothing: the transaction rolls back with the
    /// sessions in it, the same property `a_refused_removal_keeps_the_admins_sessions`
    /// pins for a removal.
    #[tokio::test]
    async fn a_refused_edit_leaves_the_sessions_alone() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        let live = store.create_session("ada", 3600).await.unwrap();
        assert!(
            store.set_disabled("ada", true).await.is_err(),
            "the last enabled admin cannot be disabled"
        );
        assert!(
            store.session_user(&live.token).await.unwrap().is_some(),
            "the rolled-back disabling must leave the live session in place"
        );
        assert!(store.set_password("ghost", "new pw").await.is_err());
        assert!(store.session_user(&live.token).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn the_plaintext_token_is_not_in_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        let store = AuthStore::open(&path).await.unwrap();
        store
            .add_user("ada", "Ada", None, Role::Admin, "hunter2")
            .await
            .unwrap();
        let s = store.create_session("ada", 3600).await.unwrap();
        let stored = store
            .query_first(
                "SELECT token_hash FROM sessions WHERE token_hash = ?1",
                vec![Value::Text(token_hash(&s.token))],
            )
            .await
            .unwrap();
        assert!(stored.is_some(), "the session is stored by its hash");

        // Read the main database *and* its sidecars. A freshly written row
        // lives in `-wal` until a checkpoint moves it, so scanning only
        // `web-auth.db` would pass without proving anything.
        let mut haystack = String::new();
        let mut files_read = 0;
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_os_string();
            p.push(suffix);
            if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(p)) {
                files_read += 1;
                haystack.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        assert!(files_read > 0, "at least the main database file must exist");
        // The hash is what proves the sweep can actually see session bytes; if
        // this fails the search covered no live data and the assertions below
        // would be vacuous.
        assert!(
            haystack.contains(&token_hash(&s.token)),
            "the sweep must reach the bytes the session was written into"
        );
        assert!(
            !haystack.contains(&s.token),
            "the raw token is never stored"
        );
        assert!(
            !haystack.contains("hunter2"),
            "the password is never stored"
        );
    }

    /// The resurrection bug: a session row that outlived its account is
    /// inherited by the next account to claim the freed name.
    #[tokio::test]
    async fn a_readded_name_does_not_inherit_the_old_holders_session() {
        let (_dir, store) = store().await;
        // Editor rather than admin: the role is incidental, and the last
        // enabled admin cannot be removed (see [`NOT_LAST_ADMIN`]).
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let old = store.create_session("ada", 3600).await.unwrap();
        store.remove_user("ada").await.unwrap();
        store
            .add_user("ada", "Ada The Second", None, Role::Viewer, "pw2")
            .await
            .unwrap();
        assert!(
            store.session_user(&old.token).await.unwrap().is_none(),
            "the previous holder's token must not resolve to the new account"
        );
    }

    /// A session row left behind by an older build (or any other route) must
    /// not become live again when the name is re-registered.
    #[tokio::test]
    async fn an_orphaned_session_row_is_swept_rather_than_inherited() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        let old = store.create_session("ada", 3600).await.unwrap();
        // Delete only the user row, exactly what the pre-fix ordering could
        // leave behind if it crashed between its two statements.
        store
            .conn
            .execute(
                "DELETE FROM users WHERE name = ?1",
                vec![Value::Text("ada".to_string())],
            )
            .await
            .unwrap();
        assert!(store.session_user(&old.token).await.unwrap().is_none());
        store
            .add_user("ada", "Ada The Second", None, Role::Viewer, "pw2")
            .await
            .unwrap();
        assert!(
            store.session_user(&old.token).await.unwrap().is_none(),
            "the orphan must have been swept, not left dormant"
        );
    }

    #[tokio::test]
    async fn remove_user_reports_a_missing_account_without_touching_anything() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        let live = store.create_session("ada", 3600).await.unwrap();
        assert!(store.remove_user("ghost").await.is_err());
        // The rollback left the unrelated account and its session intact.
        assert!(store.session_user(&live.token).await.unwrap().is_some());
        assert_eq!(store.list_users().await.unwrap().len(), 1);
    }

    /// The display name is editable, and clearing it falls back to the login
    /// name: "optional" means a client may always unset it, never that a row
    /// goes nameless.
    #[tokio::test]
    async fn display_names_are_editable_and_clearing_resets_to_the_login_name() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw")
            .await
            .unwrap();

        store
            .set_display("ada", Some("Ada Lovelace"))
            .await
            .unwrap();
        assert_eq!(store.list_users().await.unwrap()[0].display, "Ada Lovelace");

        store.set_display("ADA", None).await.unwrap();
        assert_eq!(store.list_users().await.unwrap()[0].display, "ada");

        store.set_display("ada", Some("   ")).await.unwrap();
        assert_eq!(
            store.list_users().await.unwrap()[0].display,
            "ada",
            "blank is a clear, not a display name of spaces"
        );

        assert!(store.set_display("ghost", Some("Ghost")).await.is_err());
    }

    /// Two `AuthStore` handles on one file see each other's writes as they
    /// happen, in the order the `crystalline users` CLI and the daemon do it.
    ///
    /// **Same process only, and deliberately so.** These two handles are not
    /// two processes and cannot stand in for them: turso keeps a process-wide
    /// registry of open databases keyed by file identity
    /// (`Database::lookup_in_registry`), so the second `open` here is handed
    /// the first one's `Database` back and never opens the file a second time.
    /// That makes this a test of statement ordering and visibility through one
    /// engine, which is worth having, and no evidence at all about the file
    /// locking between processes that [`open_database`] exists to solve. The
    /// test that does spawn a second process is
    /// `users_add_works_while_another_process_holds_the_auth_db` in the CLI's
    /// `tests/users.rs`, and it is the one that fails if the multiprocess WAL
    /// flag is dropped.
    #[tokio::test]
    async fn two_stores_on_one_file_interleave_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        let daemon = AuthStore::open(&path).await.unwrap();
        let cli = AuthStore::open(&path).await.unwrap();

        // The CLI adds an account; the daemon sees it without reopening.
        cli.add_user("ada", "Ada", None, Role::Viewer, "pw")
            .await
            .unwrap();
        let session = daemon.create_session("ada", 3600).await.unwrap();
        let (user, _) = daemon.session_user(&session.token).await.unwrap().unwrap();
        assert_eq!(user.role, Role::Viewer);

        // The CLI promotes; the daemon's next lookup reflects it. Editor, not
        // admin: this account is disabled and removed below, which the
        // last-admin guard would refuse (see [`NOT_LAST_ADMIN`]).
        cli.set_role("ada", Role::Editor).await.unwrap();
        let (user, _) = daemon.session_user(&session.token).await.unwrap().unwrap();
        assert_eq!(user.role, Role::Editor, "the daemon sees the CLI's write");

        // The daemon writes; the CLI reads it back.
        let second = daemon.create_session("ada", 3600).await.unwrap();
        assert!(cli.session_user(&second.token).await.unwrap().is_some());

        // The CLI disables, which revokes; the daemon no longer resolves the
        // session the CLI's write deleted.
        cli.set_disabled("ada", true).await.unwrap();
        assert!(daemon.session_user(&session.token).await.unwrap().is_none());

        // The CLI removes the account entirely, sessions and all, while the
        // daemon still holds its own handle open.
        cli.set_disabled("ada", false).await.unwrap();
        cli.remove_user("ada").await.unwrap();
        assert!(daemon.session_user(&second.token).await.unwrap().is_none());
        assert!(daemon.list_users().await.unwrap().is_empty());
    }

    /// The upgrade path: a `web-auth.db` written by a build that opened
    /// without the multiprocess WAL flag must open, and keep its accounts,
    /// under a build that opens with it.
    ///
    /// The two modes are exclusive only while a database is *live* in the
    /// other mode (turso probes both directions on open). Nothing about the
    /// file itself is mode-specific: the coordination file is a separate
    /// `-tshm` sibling created on demand, so an existing state directory needs
    /// no migration. This writes the file exactly as the previous build did,
    /// closes it and reopens it the way [`open_database`] now does.
    ///
    /// The title's promise - that it still opens, with its accounts, and stays
    /// writable - is asserted everywhere. Which mode did the opening is
    /// asserted against [`MULTIPROCESS_WAL_EXPECTED`], because on Windows
    /// multiprocess WAL is refused and the fallback is the correct outcome
    /// there, not a defect.
    #[tokio::test]
    async fn a_database_written_without_the_multiprocess_flag_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        {
            let legacy = Builder::new_local(&path.to_string_lossy())
                .build()
                .await
                .unwrap();
            let conn = legacy.connect().unwrap();
            conn.execute_batch(SCHEMA).await.unwrap();
            conn.execute(
                "INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
                 VALUES ('ada', 'Ada', NULL, 'admin', 'x', 0, '2026-01-01T00:00:00Z')",
                (),
            )
            .await
            .unwrap();
        }

        let store = AuthStore::open(&path)
            .await
            .expect("an existing legacy-mode database must open");
        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 1, "the accounts survived the mode change");
        assert_eq!(users[0].name, "ada");
        // The account is still editable, so the reopen is a real read-write
        // open and not a degraded one.
        store.set_role("ada", Role::Admin).await.unwrap();
        assert_eq!(
            path.with_file_name("web-auth.db-tshm").exists(),
            MULTIPROCESS_WAL_EXPECTED,
            "the mode that opened the file must be the one this platform can \
             have (see MULTIPROCESS_WAL_EXPECTED): multiprocess leaves the \
             coordination file beside the database, the fallback leaves none"
        );
    }

    /// The fallback in [`open_database`] hinges on recognizing turso's own
    /// "not supported" wording, which reaches us as a string rather than a
    /// typed error. A memory-like path is the one way to provoke that message
    /// without a network filesystem to hand, so it is what pins it: if a turso
    /// upgrade rewords it, this fails here, rather than the fallback silently
    /// going missing on the NFS or SMB state directory it exists for.
    #[tokio::test]
    async fn turso_still_words_an_unsupported_multiprocess_open_the_way_we_match() {
        let err = Builder::new_local(":memory:")
            .experimental_multiprocess_wal(true)
            .build()
            .await
            .expect_err("multiprocess WAL cannot be had on an in-memory path");
        assert!(
            is_multiprocess_unsupported(&err),
            "the fallback no longer recognizes turso's message: {err}"
        );
    }

    /// A locked fallback open is the one failure a user can do something
    /// about, so it must say what to do rather than hand back turso's byte
    /// range wording. The error is synthesized here because the platform that
    /// produces it (Windows, whose default IO backend has no shared WAL
    /// coordination) is not the platform this test usually runs on: the
    /// mapping is a pure function of the message, so it is testable
    /// everywhere.
    #[test]
    fn a_locked_fallback_open_says_what_holds_the_database() {
        let locked = turso::Error::Error(
            "Locking error: Failed locking file, The process cannot access the file because \
             another process has locked a portion of the file. (os error 33)"
                .to_string(),
        );
        let err = legacy_open_error(locked, Path::new("/state/web-auth.db"));
        let text = format!("{err:#}");
        assert!(
            text.contains("held by a running daemon"),
            "the message must name what holds the file: {text}"
        );
        assert!(
            text.contains("crystalline ctl shutdown"),
            "the message must name the way out: {text}"
        );
        assert!(
            text.contains("web-auth.db"),
            "the message must name the file: {text}"
        );
    }

    /// Any other reason a fallback open fails is not the locked case and must
    /// keep the plain context, so a corrupt file or a missing directory is not
    /// reported as a running daemon.
    #[test]
    fn a_fallback_open_that_fails_for_another_reason_keeps_the_plain_context() {
        let other = turso::Error::Error("file is not a database".to_string());
        let err = legacy_open_error(other, Path::new("/state/web-auth.db"));
        let text = format!("{err:#}");
        assert!(
            text.contains("opening auth database"),
            "an unrelated failure keeps the plain wording: {text}"
        );
        assert!(
            !text.contains("running daemon"),
            "an unrelated failure must not blame a daemon: {text}"
        );
    }

    /// Two simultaneous logins in the daemon are two concurrent
    /// `create_session` calls on the one shared `AuthStore`. Both open a
    /// transaction on the same connection, so without serialization the second
    /// fails outright with "cannot start a transaction within a transaction"
    /// rather than queueing. Every call must succeed and every token must work.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_sessions_on_one_store_never_fail_spuriously() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store.create_session("ada", 3600).await
            }));
        }
        let mut tokens = Vec::new();
        for task in tasks {
            let session = task
                .await
                .expect("the task must not panic")
                .expect("create_session must not fail under concurrency");
            tokens.push(session.token);
        }
        for token in &tokens {
            assert!(store.session_user(token).await.unwrap().is_some());
        }
    }

    /// The same shape with the two transactional methods mixed, plus the
    /// autocommit ones, so nothing can slip into another call's transaction.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_mixed_writes_on_one_store_never_fail_spuriously() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        // One admin nobody touches, so every task's promote-then-remove is
        // allowed: the last-admin guard's `EXISTS` subquery is thereby also
        // exercised under concurrency rather than short-circuited away.
        store
            .add_user("keeper", "Keeper", None, Role::Admin, "pw")
            .await
            .unwrap();
        for i in 0..8 {
            store
                .add_user(&format!("u{i}"), "U", None, Role::Editor, "pw")
                .await
                .unwrap();
        }

        let mut tasks = Vec::new();
        for i in 0..8 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                let name = format!("u{i}");
                store.create_session(&name, 3600).await?;
                store.set_role(&name, Role::Admin).await?;
                store.create_session(&name, 3600).await?;
                store.remove_user(&name).await?;
                store.list_users().await?;
                anyhow::Ok(())
            }));
        }
        for task in tasks {
            task.await
                .expect("the task must not panic")
                .expect("no call may fail under concurrency");
        }
        let left: Vec<String> = store
            .list_users()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert_eq!(left, vec!["keeper".to_string()]);
    }

    #[tokio::test]
    async fn delete_session_revokes_it() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw")
            .await
            .unwrap();
        let s = store.create_session("ada", 3600).await.unwrap();
        assert!(store.session_user(&s.token).await.unwrap().is_some());
        store.delete_session(&s.token).await.unwrap();
        assert!(store.session_user(&s.token).await.unwrap().is_none());
        // Logging out twice is not an error.
        store.delete_session(&s.token).await.unwrap();
    }

    #[tokio::test]
    async fn ensure_user_is_idempotent() {
        let (_dir, store) = store().await;
        let first = store
            .ensure_user("ada", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        let second = store
            .ensure_user("ada", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        assert_eq!(first.name, second.name);
        assert_eq!(first.role, second.role);
        assert_eq!(store.list_users().await.unwrap().len(), 1);
    }

    /// The provisioning cap: ensure_user refuses to mint an account past the cap,
    /// while an existing account keeps resolving whatever the count is. This is
    /// the trusted-header mitigation - a proxy misconfiguration (a header carrying
    /// a session id, say) must not mint one account per request forever.
    #[tokio::test]
    async fn ensure_user_refuses_to_mint_past_the_cap() {
        let (_dir, store) = store().await;
        store.ensure_user("ada", Role::Viewer, 2).await.unwrap();
        store.ensure_user("bob", Role::Viewer, 2).await.unwrap();

        let err = store.ensure_user("cyd", Role::Viewer, 2).await.unwrap_err();
        assert!(
            err.to_string().contains("auth.max_users"),
            "the refusal names the setting: {err}"
        );
        assert_eq!(store.list_users().await.unwrap().len(), 2);

        // Existing accounts resolve regardless of the count-vs-cap state.
        assert_eq!(
            store
                .ensure_user("ada", Role::Viewer, 2)
                .await
                .unwrap()
                .name,
            "ada"
        );
        assert_eq!(
            store
                .ensure_user("ADA", Role::Viewer, 1)
                .await
                .unwrap()
                .name,
            "ada"
        );
    }

    #[tokio::test]
    async fn ensure_user_keeps_an_admin_assigned_role() {
        let (_dir, store) = store().await;
        store
            .ensure_user("ada", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        store.set_role("ada", Role::Admin).await.unwrap();
        let again = store
            .ensure_user("ada", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        assert_eq!(again.role, Role::Admin);
    }

    #[tokio::test]
    async fn a_provisioned_user_has_no_password_to_log_in_with() {
        let (_dir, store) = store().await;
        store
            .ensure_user("ada", Role::Editor, usize::MAX)
            .await
            .unwrap();
        assert!(store.verify_password("ada", "").await.unwrap().is_none());
        store.set_password("ada", "pw").await.unwrap();
        assert!(store.verify_password("ada", "pw").await.unwrap().is_some());
    }

    /// The privilege-restoration path: a trusted header carrying a case
    /// variant must resolve to the existing account, not provision a fresh one
    /// at the default role.
    #[tokio::test]
    async fn ensure_user_folds_case_instead_of_minting_a_second_account() {
        let (_dir, store) = store().await;
        // Editor rather than admin: what matters is that a privileged, then
        // disabled, account is not restored by a case variant, and the last
        // enabled admin could not be disabled here (see [`NOT_LAST_ADMIN`]).
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        store.set_disabled("ada", true).await.unwrap();

        let same = store
            .ensure_user("Ada", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        assert_eq!(same.name, "ada");
        assert!(same.disabled, "the disable must not have been undone");
        assert_eq!(same.role, Role::Editor, "the role must not have been reset");
        assert_eq!(
            store.list_users().await.unwrap().len(),
            1,
            "no second account may exist for a case variant"
        );
    }

    #[tokio::test]
    async fn names_are_folded_and_trimmed_on_every_path() {
        let (_dir, store) = store().await;
        store
            .add_user("  AdA  ", "Ada", None, Role::Viewer, "pw")
            .await
            .unwrap();
        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "ada", "stored folded and trimmed");

        // Every read and write path accepts any casing of the same name. The
        // role moves to editor rather than admin so the removal below is not
        // refused as a lockout (see [`NOT_LAST_ADMIN`]).
        assert!(store.verify_password("ADA", "pw").await.unwrap().is_some());
        store.set_role(" Ada ", Role::Editor).await.unwrap();
        let session = store.create_session("aDa", 3600).await.unwrap();
        let (user, _) = store.session_user(&session.token).await.unwrap().unwrap();
        assert_eq!(user.role, Role::Editor);
        store.set_password("ADA", "pw2").await.unwrap();
        assert!(store.verify_password("ada", "pw2").await.unwrap().is_some());
        store.remove_user("  ADA  ").await.unwrap();
        assert!(store.list_users().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_case_variant_cannot_be_added_twice() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw")
            .await
            .unwrap();
        assert!(
            store
                .add_user("ADA", "Impostor", None, Role::Admin, "pw2")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_empty_or_blank_name_is_rejected() {
        let (_dir, store) = store().await;
        for blank in ["", "   ", "\t\n"] {
            assert!(
                store
                    .add_user(blank, "Nobody", None, Role::Viewer, "pw")
                    .await
                    .is_err(),
                "add_user must reject {blank:?}"
            );
            assert!(
                store
                    .ensure_user(blank, Role::Viewer, usize::MAX)
                    .await
                    .is_err()
            );
            // A login attempt is a `None`, not an error, like every other bad
            // credential.
            assert!(store.verify_password(blank, "pw").await.unwrap().is_none());
        }
        assert!(store.list_users().await.unwrap().is_empty());
    }

    /// Login names are space-free: the readable form belongs in the display name.
    /// Enforced in normalize_account_name so every path - add, ensure, verify, edit -
    /// refuses the same way.
    #[tokio::test]
    async fn a_name_with_internal_whitespace_is_rejected_on_every_path() {
        let (_dir, store) = store().await;
        for name in ["ada lovelace", "ada\tlovelace", "a b c"] {
            assert!(
                store
                    .add_user(name, "Ada", None, Role::Viewer, "pw")
                    .await
                    .is_err(),
                "add_user must reject {name:?}"
            );
            assert!(
                store
                    .ensure_user(name, Role::Viewer, usize::MAX)
                    .await
                    .is_err()
            );
            assert!(store.set_role(name, Role::Admin).await.is_err());
            // A login attempt is a NoHash, not an error, like other bad names.
            assert!(store.verify_password(name, "pw").await.unwrap().is_none());
        }
        assert!(store.list_users().await.unwrap().is_empty());
        // Surrounding whitespace is still merely trimmed.
        store
            .add_user("  ada  ", "Ada Lovelace", None, Role::Viewer, "pw")
            .await
            .unwrap();
        assert_eq!(store.list_users().await.unwrap()[0].name, "ada");
    }

    /// A governed domain made private at connect names who chose it. The
    /// machine owner holds no account, so its reserved name is taken without
    /// the live-account check; any other name still needs a live account,
    /// and an already-private domain is left as it stands.
    #[tokio::test]
    async fn set_governed_private_takes_the_owner_name_and_leaves_a_private_domain_alone() {
        let (_dir, store) = store().await;
        assert_eq!(
            store.set_governed_private("kb", "Owner").await.unwrap(),
            VisibilityWrite::Written
        );
        assert_eq!(
            store.domain_visibility("kb").await.unwrap().unwrap().owner,
            "owner"
        );
        assert!(store.set_governed_private("ops", "nobody").await.is_err());
        assert!(store.domain_visibility("ops").await.unwrap().is_none());
        assert_eq!(
            store.set_governed_private("kb", "owner").await.unwrap(),
            VisibilityWrite::AlreadyPrivate {
                owner: "owner".to_string()
            }
        );
    }

    /// Taking back a connect's private record deletes the acl row and no
    /// membership row.
    #[tokio::test]
    async fn take_back_governed_private_deletes_only_the_acl_row() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        store
            .add_user("bea", "Bea", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        store
            .set_domain_visibility("kb", true, "ada")
            .await
            .unwrap();
        store
            .upsert_domain_member("kb", "bea", MemberLevel::Viewer, None, "ada")
            .await
            .unwrap();
        assert!(store.take_back_governed_private("kb").await.unwrap());
        assert!(store.domain_visibility("kb").await.unwrap().is_none());
        assert_eq!(store.domain_members("kb").await.unwrap().len(), 1);
        assert!(!store.take_back_governed_private("kb").await.unwrap());
    }

    /// The machine owner's own name is not a login.
    ///
    /// `owner` is the actor key every private draft the CLI, the control socket
    /// and a local stdio agent write is filed under. An account holding it
    /// would read, edit, share and fold the machine owner's unshared work in
    /// every reviewing domain at once - in both directions, since the owner
    /// would be acting in that account's drafts too. Every path that names an
    /// account folds through `normalize_account_name`, so every one of them
    /// refuses it, whichever way it is spelled.
    #[tokio::test]
    async fn the_machine_owners_name_is_reserved_on_every_path() {
        let (_dir, store) = store().await;
        for name in ["owner", "Owner", "OWNER", "  owner  "] {
            let refusal = store
                .add_user(name, "Owner", None, Role::Admin, "pw12345678")
                .await
                .expect_err("add_user must refuse {name:?}");
            let message = refusal.to_string();
            assert!(
                message.contains("pick another name"),
                "the refusal teaches what to do instead: {message}"
            );
            assert!(
                store
                    .ensure_user(name, Role::Viewer, usize::MAX)
                    .await
                    .is_err(),
                "a trusted header cannot provision it either: {name:?}"
            );
            assert!(
                store
                    .add_first_admin(name, "Owner", "pw12345678")
                    .await
                    .is_err(),
                "and the first-run admin is not it either: {name:?}"
            );
            // The lookup paths are untouched: the machine owner's name is a
            // legitimate principal everywhere it already appears (it holds
            // drafts, it stands in a fold plan), so naming it answers "nobody
            // by that name" rather than refusing the name itself.
            assert!(store.verify_password(name, "pw").await.unwrap().is_none());
        }
        assert!(store.list_users().await.unwrap().is_empty());
        // A name that merely contains it is a name like any other.
        store
            .add_user("owner-ada", "Ada", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
    }

    #[test]
    fn normalize_account_name_trims_folds_and_rejects_empty() {
        assert_eq!(normalize_account_name("  AdA  ").unwrap(), "ada");
        assert_eq!(normalize_account_name("Ada").unwrap(), "ada");
        assert!(normalize_account_name("").is_err());
        assert!(normalize_account_name("   ").is_err());
        assert!(normalize_account_name("ada lovelace").is_err());
        // The reserved name folds like any other here; only the account
        // creation paths refuse it.
        assert_eq!(normalize_account_name("Owner").unwrap(), "owner");
        assert!(normalize_new_account_name("Owner").is_err());
    }

    #[tokio::test]
    async fn duplicate_add_errors() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        assert!(
            store
                .add_user("ada", "Ada Again", None, Role::Viewer, "pw2")
                .await
                .is_err()
        );
    }

    /// The count the first-run probe reads: zero on a fresh file, and one more
    /// for every account however it was created.
    #[tokio::test]
    async fn user_count_is_zero_until_an_account_exists() {
        let (_dir, store) = store().await;
        assert_eq!(store.user_count().await.unwrap(), 0);
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        assert_eq!(store.user_count().await.unwrap(), 1);
        // The trusted-header path mints accounts too, and they count the same.
        store
            .ensure_user("bob", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        assert_eq!(store.user_count().await.unwrap(), 2);
        store.remove_user("bob").await.unwrap();
        assert_eq!(store.user_count().await.unwrap(), 1);
    }

    /// The first admin is created only into an empty table, and the account it
    /// creates is a real one: admin, enabled, folded name, display as typed,
    /// and a password that verifies.
    #[tokio::test]
    async fn add_first_admin_creates_one_admin_and_only_on_an_empty_table() {
        let (_dir, store) = store().await;
        assert!(
            store.add_first_admin("Ada", "Ada", "s3cret").await.unwrap(),
            "an empty table is what the first-run path is for"
        );
        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "ada", "the name is folded like every other");
        assert_eq!(users[0].display, "Ada");
        assert_eq!(users[0].role, Role::Admin);
        assert!(!users[0].disabled);
        assert!(matches!(
            store.check_password("ada", "s3cret").await.unwrap(),
            PasswordCheck::Verified(_)
        ));

        assert!(
            !store.add_first_admin("bob", "Bob", "pw").await.unwrap(),
            "the slot is gone once any account exists"
        );
        assert_eq!(
            store.user_count().await.unwrap(),
            1,
            "and nothing was written"
        );
    }

    /// The account that closes the slot need not be an admin, and need not have
    /// been created here: `crystalline users add` in another process is the
    /// case that matters, and it lands an ordinary row.
    #[tokio::test]
    async fn add_first_admin_refuses_once_any_account_exists() {
        let (_dir, added) = store().await;
        added
            .add_user("vera", "Vera", None, Role::Viewer, "pw")
            .await
            .unwrap();
        assert!(
            !added
                .add_first_admin("root", "Root", "rootpw")
                .await
                .unwrap()
        );
        assert_eq!(added.list_users().await.unwrap().len(), 1);

        let (_dir2, provisioned) = store().await;
        provisioned
            .ensure_user("proxied", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        assert!(
            !provisioned
                .add_first_admin("root", "Root", "rootpw")
                .await
                .unwrap()
        );
    }

    /// An unusable name is refused before anything is written, so a typo does
    /// not consume the one slot there is.
    #[tokio::test]
    async fn add_first_admin_refuses_a_name_the_store_cannot_key_on() {
        let (_dir, store) = store().await;
        assert!(store.add_first_admin("  ", "Blank", "pw").await.is_err());
        assert!(
            store
                .add_first_admin("ada lovelace", "Ada", "pw")
                .await
                .is_err()
        );
        assert_eq!(store.user_count().await.unwrap(), 0);
        assert!(store.add_first_admin("ada", "Ada", "pw").await.unwrap());
    }

    /// Invariant 1's real pin: the claim holds across PROCESSES, which is what
    /// the REST layer's own guard mutex can never show.
    ///
    /// Two [`AuthStore`]s are opened on one `web-auth.db` - the exact shape
    /// `crystalline users add` takes while the daemon serves - and the first
    /// admin is raced against a second first-admin call and against a plain
    /// `add_user`. Two assertions carry the invariant, and neither is "exactly
    /// one row lands": in the `add_user` leg two rows legitimately can, since
    /// an ordinary add of a different name is not competing for the slot at
    /// all. What must hold is that `add_first_admin` reports success at most
    /// once, and never once any row already exists.
    ///
    /// **The two racing calls are `tokio::spawn`ed behind a barrier, and the
    /// race is repeated ten times.** All three details are load bearing and
    /// none is style. A `tokio::join!` polls both futures on ONE task, so they
    /// can only interleave where one of them returns `Pending` - and the
    /// check-then-insert window this test exists to catch is store work that
    /// never yields, so a `join!` version of this test passes against exactly
    /// the naive implementation the plan rejects (measured against a
    /// deliberately naive store: 12 runs, 12 misses). Spawned onto different
    /// worker threads and released together, the same race caught that store in
    /// roughly four rounds out of five, and ten rounds is what turns "roughly
    /// four out of five" into a pin. Anyone tempted to tidy this back into a
    /// `join!` is removing the only test that can see the bug.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_second_store_open_cannot_also_win_first_admin() {
        use std::sync::Arc;

        // Leg one: first admin against first admin, on two opens of one file.
        for round in 0..10 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("web-auth.db");
            let daemon = Arc::new(AuthStore::open(&path).await.unwrap());
            let cli = Arc::new(AuthStore::open(&path).await.unwrap());
            // Released together, so what separates the two calls is the work
            // itself rather than however long each task waited to be scheduled.
            let gate = Arc::new(tokio::sync::Barrier::new(2));
            let one = tokio::spawn({
                let store = daemon.clone();
                let gate = gate.clone();
                async move {
                    gate.wait().await;
                    store.add_first_admin("root", "Root", "rootpw").await
                }
            });
            let two = tokio::spawn({
                let store = cli.clone();
                let gate = gate.clone();
                async move {
                    gate.wait().await;
                    store.add_first_admin("boss", "Boss", "bosspw").await
                }
            });
            let won = [one.await.unwrap().unwrap(), two.await.unwrap().unwrap()];
            assert_eq!(
                won.iter().filter(|w| **w).count(),
                1,
                "round {round}: exactly one of two racing first-admin calls may win"
            );
            assert_eq!(
                daemon.user_count().await.unwrap(),
                1,
                "round {round}: and exactly one row is what they left behind"
            );
        }

        // Leg two: first admin against an ordinary add from the other open.
        // Both may land - the names differ and `users add` is not competing for
        // the slot - but a first admin created after a row exists would be a
        // check-then-insert that read stale, and a first admin that reported
        // failure while writing a row would be worse still.
        for round in 0..10 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("web-auth.db");
            let daemon = Arc::new(AuthStore::open(&path).await.unwrap());
            let cli = Arc::new(AuthStore::open(&path).await.unwrap());
            let gate = Arc::new(tokio::sync::Barrier::new(2));
            let first = tokio::spawn({
                let store = daemon.clone();
                let gate = gate.clone();
                async move {
                    gate.wait().await;
                    store.add_first_admin("root", "Root", "rootpw").await
                }
            });
            let added = tokio::spawn({
                let store = cli.clone();
                let gate = gate.clone();
                async move {
                    gate.wait().await;
                    store.add_user("ada", "Ada", None, Role::Viewer, "pw").await
                }
            });
            added.await.unwrap().unwrap();
            // Both racers have finished before the table is read, or the read
            // could miss a row that was still on its way in.
            let first = first.await.unwrap().unwrap();
            let names: Vec<String> = daemon
                .list_users()
                .await
                .unwrap()
                .into_iter()
                .map(|u| u.name)
                .collect();
            if first {
                assert!(names.contains(&"root".to_string()), "round {round}");
            } else {
                assert_eq!(
                    names,
                    vec!["ada".to_string()],
                    "round {round}: a first admin that reported failure must not \
                     have written a row"
                );
            }
            assert!(
                !daemon.add_first_admin("late", "Late", "pw").await.unwrap(),
                "round {round}: and the slot stays shut for every later caller"
            );
        }
    }

    #[tokio::test]
    async fn edits_to_an_unknown_user_error() {
        let (_dir, store) = store().await;
        assert!(store.set_password("ghost", "pw").await.is_err());
        assert!(store.set_role("ghost", Role::Admin).await.is_err());
        assert!(store.set_disabled("ghost", true).await.is_err());
        assert!(store.remove_user("ghost").await.is_err());
        assert!(store.create_session("ghost", 60).await.is_err());
    }

    #[tokio::test]
    async fn remove_user_takes_its_sessions_with_it() {
        let (_dir, store) = store().await;
        // Editor rather than admin: the role is incidental, and the last
        // enabled admin cannot be removed (see [`NOT_LAST_ADMIN`]).
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        let s = store.create_session("ada", 3600).await.unwrap();
        store.remove_user("ada").await.unwrap();
        assert!(store.session_user(&s.token).await.unwrap().is_none());
        assert!(store.list_users().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_users_is_sorted_and_reopens_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("web-auth.db");
        {
            let store = AuthStore::open(&path).await.unwrap();
            store
                .add_user("zoe", "Zoe", None, Role::Viewer, "pw")
                .await
                .unwrap();
            store
                .add_user("ada", "Ada", None, Role::Admin, "pw")
                .await
                .unwrap();
        }
        let reopened = AuthStore::open(&path).await.unwrap();
        let names: Vec<String> = reopened
            .list_users()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert_eq!(names, vec!["ada".to_string(), "zoe".to_string()]);
    }

    /// The lockout guard: the last enabled admin cannot be removed, disabled
    /// or demoted, so an installation cannot lose its last way in. Enforced in
    /// the store rather than in the `crystalline users` CLI because a
    /// check-then-act in the CLI would race a second invocation.
    #[tokio::test]
    async fn the_last_admin_cannot_be_removed_disabled_or_demoted() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        // A non-admin account is no help: it cannot administer anything.
        store
            .add_user("bob", "Bob", None, Role::Editor, "pw")
            .await
            .unwrap();

        for err in [
            store.remove_user("ada").await.unwrap_err(),
            store.set_disabled("ada", true).await.unwrap_err(),
            store.set_role("ada", Role::Viewer).await.unwrap_err(),
        ] {
            assert!(
                err.to_string().contains("last admin"),
                "expected a last-admin refusal, got: {err}"
            );
        }

        // Nothing was applied: the account is still an enabled admin.
        let users = store.list_users().await.unwrap();
        let ada = users.iter().find(|u| u.name == "ada").unwrap();
        assert_eq!(ada.role, Role::Admin);
        assert!(!ada.disabled);
    }

    /// With a second enabled admin in place, all three operations go through.
    #[tokio::test]
    async fn an_admin_can_be_removed_disabled_and_demoted_beside_another_admin() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        store
            .add_user("bob", "Bob", None, Role::Admin, "pw")
            .await
            .unwrap();
        store
            .add_user("cyd", "Cyd", None, Role::Admin, "pw")
            .await
            .unwrap();
        store
            .add_user("dee", "Dee", None, Role::Admin, "pw")
            .await
            .unwrap();

        // Each edit leaves at least one enabled admin behind, so each is
        // allowed: four admins, then a disable, a demotion and a removal.
        store.set_disabled("ada", true).await.unwrap();
        store.set_role("bob", Role::Viewer).await.unwrap();
        store.remove_user("cyd").await.unwrap();

        // Re-enabling the disabled admin is never refused.
        store.set_disabled("ada", false).await.unwrap();
        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 3);
        assert!(users.iter().any(|u| u.name == "ada" && !u.disabled));
    }

    /// A disabled admin cannot log in, so it is not a way back in and must not
    /// satisfy the guard for the one admin that still works.
    #[tokio::test]
    async fn a_disabled_admin_does_not_count_as_a_remaining_admin() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        store
            .add_user("bob", "Bob", None, Role::Admin, "pw")
            .await
            .unwrap();
        store.set_disabled("bob", true).await.unwrap();

        assert!(store.remove_user("ada").await.is_err());
        assert!(store.set_disabled("ada", true).await.is_err());
        assert!(store.set_role("ada", Role::Editor).await.is_err());

        // The already-disabled admin is not the last *enabled* one, so it is
        // not itself protected.
        store.remove_user("bob").await.unwrap();
    }

    /// The guard only applies to enabled admins. Every other account, and
    /// every promotion, is untouched by it.
    #[tokio::test]
    async fn the_guard_leaves_non_admins_and_promotions_alone() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        store
            .add_user("bob", "Bob", None, Role::Viewer, "pw")
            .await
            .unwrap();
        // The one and only viewer is not an admin: no protection.
        store.set_disabled("bob", true).await.unwrap();
        store.set_disabled("bob", false).await.unwrap();
        // Promoting is always allowed, even for the last admin itself.
        store.set_role("ada", Role::Admin).await.unwrap();
        store.set_role("bob", Role::Admin).await.unwrap();
        // Now that there are two, the first may go.
        store.remove_user("ada").await.unwrap();
    }

    /// A refused removal must not take the account's sessions with it: the
    /// whole operation rolls back, not just the delete that was refused.
    #[tokio::test]
    async fn a_refused_removal_keeps_the_admins_sessions() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        let live = store.create_session("ada", 3600).await.unwrap();
        assert!(store.remove_user("ada").await.is_err());
        assert!(
            store.session_user(&live.token).await.unwrap().is_some(),
            "the rolled-back removal must leave the live session in place"
        );
    }

    /// The guard must not swallow a plain typo: an unknown name still reports
    /// itself as unknown rather than as a lockout refusal.
    #[tokio::test]
    async fn an_unknown_name_still_reports_itself_as_unknown() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        for err in [
            store.remove_user("ghost").await.unwrap_err(),
            store.set_disabled("ghost", true).await.unwrap_err(),
            store.set_role("ghost", Role::Viewer).await.unwrap_err(),
        ] {
            assert!(
                err.to_string().contains("no such user"),
                "expected an unknown-user error, got: {err}"
            );
        }
    }

    #[test]
    fn roles_round_trip_through_text_and_json() {
        for role in [Role::Viewer, Role::Editor, Role::Admin] {
            assert_eq!(role.as_str().parse::<Role>().unwrap(), role);
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                serde_json::Value::String(role.as_str().to_string())
            );
        }
        assert!("root".parse::<Role>().is_err());
        // A hand-edited or corrupt row resolves to the least privileged role.
        assert_eq!(role_from_db("root"), Role::Viewer);
    }

    /// The column migration: a database created by the slice-1 schema (no
    /// last_seen_at) opens cleanly and gains the column, and opening twice is
    /// harmless - the idempotent-open contract, extended to columns.
    #[tokio::test]
    async fn an_old_database_gains_the_last_seen_column_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        // Hand-create the pre-migration shape.
        {
            let db = Builder::new_local(&path.to_string_lossy())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            conn.execute_batch(
                "CREATE TABLE users (
                    name TEXT PRIMARY KEY,
                    display TEXT NOT NULL,
                    email TEXT,
                    role TEXT NOT NULL,
                    pass_hash TEXT,
                    disabled INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL
                );
                INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
                VALUES ('ada', 'Ada', NULL, 'admin', NULL, 0, '2026-01-01T00:00:00Z');",
            )
            .await
            .unwrap();
        }
        let store = AuthStore::open(&path).await.unwrap();
        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "ada");
        assert!(users[0].last_seen.is_none(), "never seen yet");
        drop(store);
        // Re-opening (the migrated shape) must not fail on the duplicate column.
        let again = AuthStore::open(&path).await.unwrap();
        assert_eq!(again.list_users().await.unwrap().len(), 1);
    }

    /// The two Reflection columns: a file written before them opens, reads
    /// every account and membership as `default`, and keeps what is set.
    #[tokio::test]
    async fn the_reflection_levels_are_stored_and_an_older_file_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        {
            // The shape before 0.26.0: no `reflection` column anywhere.
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            // The per-domain grants table came later still (0.30.0).
            let grants = SCHEMA
                .find("CREATE TABLE IF NOT EXISTS domain_reflection_grant")
                .unwrap();
            let old = format!(
                "{}{}",
                &SCHEMA[..grants],
                SCHEMA[grants..].split_once(";\n").unwrap().1
            );
            let old = old.replace("    reflection TEXT,\n", "");
            assert!(!old.contains("reflection"), "both columns are left out");
            conn.execute_batch(&old).await.unwrap();
        }
        let auth = AuthStore::open(&path).await.unwrap();
        auth.add_user("ann", "Ann", None, Role::None, "pw12345678")
            .await
            .unwrap();
        auth.add_user("keeper", "Keeper", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        assert_eq!(
            auth.user("ann").await.unwrap().unwrap().reflection,
            ReflectionLevel::Default
        );
        assert_eq!(auth.user("ann").await.unwrap().unwrap().role, Role::None);
        // The grants table is made on open.
        auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "keeper")
            .await
            .unwrap();
        auth.set_reflection_level("ann", ReflectionLevel::Contributor)
            .await
            .unwrap();
        assert_eq!(
            auth.user("ann").await.unwrap().unwrap().reflection,
            ReflectionLevel::Contributor
        );
        auth.set_reflection_level("ann", ReflectionLevel::Default)
            .await
            .unwrap();
        // Created with a level: one write, read back as given.
        auth.add_user_with_reflection(
            "bea",
            "Bea",
            None,
            Role::None,
            ReflectionLevel::Reader,
            "pw12345678",
        )
        .await
        .unwrap();
        assert_eq!(
            auth.user("bea").await.unwrap().unwrap().reflection,
            ReflectionLevel::Reader
        );
        auth.add_token_only_user_with_reflection(
            "bot",
            "bot",
            None,
            Role::None,
            ReflectionLevel::Contributor,
            "bot",
        )
        .await
        .unwrap();
        assert_eq!(
            auth.user("bot").await.unwrap().unwrap().reflection,
            ReflectionLevel::Contributor
        );
        assert_eq!(
            auth.user("ann").await.unwrap().unwrap().reflection,
            ReflectionLevel::Default
        );

        auth.set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let err = auth
            .upsert_domain_member("lab", "ann", MemberLevel::None, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::EmptyMembership),
            "{err:#}"
        );
        auth.upsert_domain_member(
            "lab",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Reader),
            "keeper",
        )
        .await
        .unwrap();
        let members = auth.domain_members("lab").await.unwrap();
        assert_eq!(
            (members[0].level, members[0].reflection),
            (MemberLevel::None, ReflectionLevel::Reader)
        );
        // Changing one axis never resets the other: no Reflection level given
        // keeps the stored one.
        auth.upsert_domain_member("lab", "ann", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        let members = auth.domain_members("lab").await.unwrap();
        assert_eq!(
            (members[0].level, members[0].reflection),
            (MemberLevel::Viewer, ReflectionLevel::Reader)
        );
        let err = auth
            .upsert_domain_member(
                "lab",
                "ann",
                MemberLevel::None,
                Some(ReflectionLevel::Default),
                "keeper",
            )
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::EmptyMembership),
            "{err:#}"
        );
        assert_eq!(
            auth.member_reflection("lab", "ann").await.unwrap(),
            ReflectionLevel::Reader
        );
        assert_eq!(
            auth.member_reflection("lab", "keeper").await.unwrap(),
            ReflectionLevel::Default
        );
        drop(auth);
        // Re-opening the migrated file does not fail on the duplicate columns.
        AuthStore::open(&path).await.unwrap();
    }

    fn entries(items: &[&str]) -> Vec<String> {
        items.iter().map(|e| e.to_string()).collect()
    }

    /// The table is created on open over a database an older build wrote,
    /// and opening twice is harmless.
    #[tokio::test]
    async fn an_old_database_gains_the_write_list_table_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        {
            let db = Builder::new_local(&path.to_string_lossy())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            conn.execute_batch(
                "CREATE TABLE users (
                    name TEXT PRIMARY KEY,
                    display TEXT NOT NULL,
                    email TEXT,
                    role TEXT NOT NULL,
                    pass_hash TEXT,
                    disabled INTEGER NOT NULL DEFAULT 0,
                    last_seen_at TEXT,
                    created_at TEXT NOT NULL
                );
                INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
                VALUES ('bot', 'bot', NULL, 'editor', NULL, 0, '2026-01-01T00:00:00Z');",
            )
            .await
            .unwrap();
        }
        let store = AuthStore::open(&path).await.unwrap();
        assert_eq!(store.write_list("bot").await.unwrap(), None);
        store
            .set_write_list("bot", &entries(&["agents-bot"]))
            .await
            .unwrap();
        drop(store);
        let again = AuthStore::open(&path).await.unwrap();
        assert_eq!(
            again.write_list("bot").await.unwrap(),
            Some(entries(&["agents-bot"]))
        );
    }

    #[tokio::test]
    async fn a_write_list_is_set_replaced_read_and_cleared() {
        let (_dir, store) = store().await;
        store
            .add_user("bot", "Bot", None, Role::Editor, "pw")
            .await
            .unwrap();
        assert_eq!(store.write_list("bot").await.unwrap(), None);
        store
            .set_write_list(
                "BOT",
                &entries(&[" shared-* ", "agents-bot", "agents-bot", ""]),
            )
            .await
            .unwrap();
        assert_eq!(
            store.write_list("bot").await.unwrap(),
            Some(entries(&["agents-bot", "shared-*"])),
            "trimmed, deduplicated, empties dropped, sorted, keyed on the folded name"
        );
        store
            .set_write_list("bot", &entries(&["other"]))
            .await
            .unwrap();
        assert_eq!(
            store.write_list("bot").await.unwrap(),
            Some(entries(&["other"])),
            "a set replaces the whole list"
        );
        store.clear_write_list("bot").await.unwrap();
        assert_eq!(store.write_list("bot").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_write_list_needs_an_account_and_an_entry() {
        let (_dir, store) = store().await;
        store
            .add_user("bot", "Bot", None, Role::Editor, "pw")
            .await
            .unwrap();
        let ghost = store
            .set_write_list("ghost", &entries(&["open"]))
            .await
            .unwrap_err();
        assert!(
            format!("{ghost:#}").contains("no such user: 'ghost'"),
            "{ghost:#}"
        );
        let ghost = store.clear_write_list("ghost").await.unwrap_err();
        assert!(
            format!("{ghost:#}").contains("no such user: 'ghost'"),
            "{ghost:#}"
        );
        let empty = store
            .set_write_list("bot", &entries(&["", "  "]))
            .await
            .unwrap_err();
        assert!(
            format!("{empty:#}").contains("at least one entry"),
            "{empty:#}"
        );
        assert_eq!(store.write_list("bot").await.unwrap(), None);
    }

    /// A rename rewrites exact entries and leaves `*` entries alone, on a
    /// shared domain too (which has no acl, member or grant row), keeps an
    /// entry that already named the new name, and a second run changes
    /// nothing.
    #[tokio::test]
    async fn a_rename_follows_exact_write_entries_only() {
        let (_dir, store) = store().await;
        for name in ["bot", "both"] {
            store
                .add_user(name, name, None, Role::Editor, "pw")
                .await
                .unwrap();
        }
        store
            .set_write_list("bot", &entries(&["eng", "eng-*", "open"]))
            .await
            .unwrap();
        store
            .set_write_list("both", &entries(&["eng", "platform"]))
            .await
            .unwrap();
        store.rename_domain("eng", "platform").await.unwrap();
        assert_eq!(
            store.write_list("bot").await.unwrap(),
            Some(entries(&["eng-*", "open", "platform"]))
        );
        assert_eq!(
            store.write_list("both").await.unwrap(),
            Some(entries(&["platform"])),
            "an account that named both ends keeps one entry"
        );
        store.rename_domain("eng", "platform").await.unwrap();
        assert_eq!(
            store.write_list("bot").await.unwrap(),
            Some(entries(&["eng-*", "open", "platform"])),
            "idempotent"
        );
    }

    /// The list goes with the account, on both removal paths, so a name
    /// added again starts with no list.
    #[tokio::test]
    async fn removing_an_account_removes_its_write_list() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        for name in ["bot", "bot2"] {
            store
                .add_user(name, name, None, Role::Editor, "pw")
                .await
                .unwrap();
            store
                .set_write_list(name, &entries(&["agents-bot"]))
                .await
                .unwrap();
        }
        store.remove_user("bot").await.unwrap();
        store.remove_user_force("bot2").await.unwrap();
        for name in ["bot", "bot2"] {
            store
                .add_user(name, name, None, Role::Editor, "pw")
                .await
                .unwrap();
            assert_eq!(store.write_list(name).await.unwrap(), None, "{name}");
        }
    }

    /// Resolving a session stamps the account as seen; a trusted-header contact
    /// (ensure_user) stamps it too.
    #[tokio::test]
    async fn resolving_a_session_updates_last_seen() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw")
            .await
            .unwrap();
        assert!(store.list_users().await.unwrap()[0].last_seen.is_none());

        let s = store.create_session("ada", 3600).await.unwrap();
        store.session_user(&s.token).await.unwrap().unwrap();
        let seen = store.list_users().await.unwrap()[0].last_seen.clone();
        assert!(seen.is_some(), "a resolved session is a sighting");

        let provisioned = store
            .ensure_user("bob", Role::Viewer, usize::MAX)
            .await
            .unwrap();
        assert!(
            provisioned.last_seen.is_some(),
            "provisioning is a sighting too"
        );
    }

    #[tokio::test]
    async fn marking_a_sighting_moves_last_seen() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw")
            .await
            .unwrap();
        let before = store.user("ada").await.unwrap().unwrap().last_seen;
        assert!(before.is_none(), "an account nobody has used yet");
        store.mark_seen("ada").await.unwrap();
        let after = store.user("ada").await.unwrap().unwrap().last_seen;
        assert!(after.is_some(), "and one that just arrived");
        // A name nobody holds is not an error: the caller stamps a row it read,
        // and a row removed in between is somebody else's problem to report.
        store.mark_seen("nobody").await.unwrap();
    }

    /// The operator escape hatch: --force bypasses the last-admin guard. It still
    /// reports a missing account, and a forced removal still takes the sessions
    /// with it in the same transaction.
    #[tokio::test]
    async fn forced_edits_bypass_the_last_admin_guard() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        let live = store.create_session("ada", 3600).await.unwrap();

        // The guarded paths refuse; the forced ones do not.
        assert!(store.set_role("ada", Role::Viewer).await.is_err());
        store.set_role_force("ada", Role::Viewer).await.unwrap();
        assert_eq!(store.list_users().await.unwrap()[0].role, Role::Viewer);

        store.set_role_force("ada", Role::Admin).await.unwrap();
        assert!(store.remove_user("ada").await.is_err());
        store.remove_user_force("ada").await.unwrap();
        assert!(store.list_users().await.unwrap().is_empty());
        assert!(
            store.session_user(&live.token).await.unwrap().is_none(),
            "a forced removal still revokes the sessions"
        );

        assert!(store.set_role_force("ghost", Role::Viewer).await.is_err());
        assert!(store.remove_user_force("ghost").await.is_err());
    }

    #[tokio::test]
    async fn mcp_token_round_trip_and_revocation() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let issued = store.issue_mcp_token("ada", "laptop-agent").await.unwrap();
        assert!(issued.token.starts_with("cmt_"));
        assert_eq!(issued.token.len(), 4 + 64);
        let user = store.mcp_token_user(&issued.token).await.unwrap().unwrap();
        assert_eq!(user.name, "ada");
        let listed = store.list_mcp_tokens("ada").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].last_used.is_some());

        // The store never keeps the plaintext - proved, not just commented.
        // Read the raw column via the store's own connection (tests are a
        // descendant module, so the private field and helper are reachable)
        // and check it against an independently computed sha256, the same
        // property `token_hash` is supposed to have.
        let row = store
            .query_first(
                "SELECT token_hash FROM mcp_tokens WHERE id = ?1",
                vec![Value::Integer(issued.id)],
            )
            .await
            .unwrap()
            .unwrap();
        let stored_hash = cell_text(&row, 0).unwrap();
        assert_eq!(stored_hash.len(), 64, "a sha256 hex digest is 64 chars");
        assert!(
            stored_hash.chars().all(|c| c.is_ascii_hexdigit()),
            "the stored value must be hex, not the token itself"
        );
        assert_ne!(stored_hash, issued.token);
        assert!(!stored_hash.contains(issued.token.as_str()));
        assert!(!issued.token.contains(stored_hash.as_str()));
        let mut hasher = Sha256::new();
        hasher.update(issued.token.as_bytes());
        let expected = crystalline_index::hex_lower(&hasher.finalize());
        assert_eq!(
            stored_hash, expected,
            "the column holds sha256 of the whole token, prefix included"
        );

        assert!(store.revoke_mcp_token("ada", issued.id).await.unwrap());
        assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_token_only_account_has_no_password_and_one_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        let issued = store
            .add_token_only_user("Agent-Build", "Agent-Build", None, Role::Editor, "build")
            .await
            .unwrap();
        assert!(issued.token.starts_with("cmt_"));
        assert_eq!(issued.token.len(), 4 + 64);
        assert_eq!(issued.label, "build");

        for attempt in ["", "pw12345678", "agent-build"] {
            assert!(
                store
                    .verify_password("agent-build", attempt)
                    .await
                    .unwrap()
                    .is_none(),
                "password sign-in never succeeds for a token-only account"
            );
        }
        assert!(matches!(
            store.check_password("agent-build", "x").await.unwrap(),
            PasswordCheck::NoHash
        ));

        let user = store.mcp_token_user(&issued.token).await.unwrap().unwrap();
        assert_eq!(user.name, "agent-build");
        assert_eq!(user.role, Role::Editor);
        assert_eq!(store.list_mcp_tokens("agent-build").await.unwrap().len(), 1);
        assert!(
            store
                .passwordless_users()
                .await
                .unwrap()
                .contains("agent-build")
        );
        let debug = format!("{issued:?}");
        assert!(!debug.contains(&issued.token), "{debug}");
    }

    #[tokio::test]
    async fn a_token_only_account_with_a_taken_name_is_refused_and_issues_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        let err = store
            .add_token_only_user("ADA", "Ada", None, Role::Editor, "build")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("UNIQUE constraint"), "{err:#}");
        assert!(store.list_mcp_tokens("ada").await.unwrap().is_empty());
        let ada = store.user("ada").await.unwrap().unwrap();
        assert_eq!(ada.role, Role::Viewer, "the existing account is untouched");
        assert!(
            store
                .verify_password("ada", "pw12345678")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_failed_token_issue_leaves_no_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let taken = store.issue_mcp_token("ada", "laptop").await.unwrap();
        // The token hash is UNIQUE, so reusing a live token makes the second
        // insert fail after the account insert succeeded.
        let err = store
            .add_token_only_user_with(
                "agent-build",
                "agent-build",
                None,
                Role::Editor,
                ReflectionLevel::Default,
                "build",
                taken.token.clone(),
            )
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("UNIQUE constraint"), "{err:#}");
        assert!(
            store.user("agent-build").await.unwrap().is_none(),
            "the account was rolled back with the token"
        );
        assert_eq!(store.list_mcp_tokens("ada").await.unwrap().len(), 1);
        // The name is free again.
        store
            .add_token_only_user("agent-build", "agent-build", None, Role::Editor, "build")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mcp_token_of_a_disabled_account_stops_resolving() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let issued = store.issue_mcp_token("ada", "t").await.unwrap();
        store.set_disabled("ada", true).await.unwrap();
        assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());

        // Disabled without deletion: the row must still be there, and
        // re-enabling must hand the very same token back rather than force a
        // re-issue.
        let listed = store.list_mcp_tokens("ada").await.unwrap();
        assert_eq!(
            listed.len(),
            1,
            "disabling an account must not delete its mcp tokens"
        );
        store.set_disabled("ada", false).await.unwrap();
        let user = store.mcp_token_user(&issued.token).await.unwrap().unwrap();
        assert_eq!(user.name, "ada");
    }

    #[tokio::test]
    async fn an_orphaned_mcp_token_row_cannot_resolve_and_gets_pruned() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let issued = store.issue_mcp_token("ada", "agent").await.unwrap();

        // Simulate the stranded-row scenario `issue_mcp_token`'s own
        // transaction now prevents live: an account vanishing by a path other
        // than `remove_user`'s sweep, leaving an `mcp_tokens` row with no
        // matching account (the table carries no foreign key). A row written
        // before this defense existed, or reached some other way, must still
        // fail closed rather than resolve for whoever next claims the name.
        store
            .conn
            .execute("DELETE FROM users WHERE name = 'ada'", ())
            .await
            .unwrap();

        assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());

        let remaining = store
            .query_first("SELECT COUNT(*) FROM mcp_tokens", vec![])
            .await
            .unwrap()
            .unwrap();
        let count = match remaining.get_value(0) {
            Ok(Value::Integer(n)) => n,
            other => panic!("unexpected COUNT(*) result: {other:?}"),
        };
        assert_eq!(
            count, 0,
            "mcp_token_user must prune the orphaned row, not just refuse it"
        );
    }

    #[tokio::test]
    async fn mcp_token_rotation_replaces_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let first = store.issue_mcp_token("ada", "agent").await.unwrap();
        let second = store.rotate_mcp_token("ada", first.id).await.unwrap();
        assert_eq!(second.label, "agent");
        assert!(store.mcp_token_user(&first.token).await.unwrap().is_none());
        assert!(store.mcp_token_user(&second.token).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn removing_a_user_revokes_its_mcp_tokens() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let issued = store.issue_mcp_token("ada", "agent").await.unwrap();
        store.remove_user("ada").await.unwrap();
        assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());
        assert!(store.list_mcp_tokens("ada").await.unwrap().is_empty());
    }

    /// A name that is not an account reads as `None`, and a folded spelling of
    /// one that is finds it: the difference between "no such user" and "an
    /// account with no tokens" rests on this.
    #[tokio::test]
    async fn one_account_is_read_back_by_any_spelling_of_its_name() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        for spelling in ["ada", "ADA", "  Ada  "] {
            let found = store.user(spelling).await.unwrap();
            assert_eq!(found.expect("'{spelling}' is ada").name, "ada");
        }
        assert!(store.user("ghost").await.unwrap().is_none());
    }

    /// The one unhashed copy of a live credential must never be one
    /// `tracing::debug!` or one failed assertion away from a log file, while
    /// the id and the label - the parts that make such a line useful - still
    /// print. Asserted on the random half rather than on the whole token: the
    /// redaction keeps the [`MCP_TOKEN_PREFIX`], so `!contains(&issued.token)`
    /// would pass even if the secret leaked in pieces.
    #[tokio::test]
    async fn an_issued_token_never_prints_its_secret() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let issued = store.issue_mcp_token("ada", "laptop").await.unwrap();
        let secret = issued
            .token
            .strip_prefix(MCP_TOKEN_PREFIX)
            .expect("a token carries the prefix");
        let text = format!("{issued:?}");
        assert!(!text.contains(secret), "the secret is redacted: {text}");
        assert!(text.contains("redacted"), "and says so: {text}");
        assert!(
            text.contains("laptop") && text.contains(&issued.id.to_string()),
            "while the id and label still print: {text}"
        );
    }

    /// The membership cast every test below shares: two accounts that own or
    /// join things and one that never does.
    async fn members_cast(store: &AuthStore) {
        for (name, role) in [
            ("keeper", Role::Editor),
            ("mem", Role::Viewer),
            ("out", Role::Editor),
        ] {
            store
                .add_user(name, name, None, role, "pw12345678")
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_domain_is_shared_until_somebody_makes_it_private() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        assert!(store.domain_visibility("lab").await.unwrap().is_none());
        assert!(store.private_domains().await.unwrap().is_empty());
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let acl = store.domain_visibility("lab").await.unwrap().unwrap();
        assert_eq!(acl.domain, "lab");
        assert_eq!(acl.owner, "keeper");
        assert_eq!(
            store.private_domains().await.unwrap(),
            vec![DomainAcl {
                domain: "lab".into(),
                owner: "keeper".into()
            }]
        );
        // The name is trimmed but never folded: the engine keys its domain map
        // on the literal name.
        assert!(store.domain_visibility("  lab  ").await.unwrap().is_some());
        assert!(store.domain_visibility("Lab").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_domain_acl_needs_a_live_owner() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        let err = store
            .set_domain_visibility("lab", true, "ghost")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such user"), "{err}");
        store.set_disabled("mem", true).await.unwrap();
        let err = store
            .set_domain_visibility("lab", true, "mem")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("disabled"), "{err}");
        assert!(
            store.domain_visibility("lab").await.unwrap().is_none(),
            "a refused call writes nothing"
        );
    }

    #[tokio::test]
    async fn making_a_domain_shared_again_takes_its_members_with_it() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        assert_eq!(store.domain_members("lab").await.unwrap().len(), 1);
        store
            .set_domain_visibility("lab", false, "keeper")
            .await
            .unwrap();
        assert!(store.domain_visibility("lab").await.unwrap().is_none());
        assert!(
            store.domain_members("lab").await.unwrap().is_empty(),
            "the members go with the acl, so making it private again does not \
             restore somebody else's invitations"
        );
        assert!(store.memberships_of("mem").await.unwrap().is_empty());
        // Asking for a state that already holds is not an error.
        store
            .set_domain_visibility("lab", false, "keeper")
            .await
            .unwrap();
    }

    /// A domain rename carries every record the accounts database keeps under
    /// the old name: the acl row with its owner, the membership rows and the
    /// draft share-links. A second call finds nothing under the old name and is
    /// fine, which is what a rename completed again after a crash does.
    #[tokio::test]
    async fn renaming_a_domain_moves_its_acl_members_and_grants() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("eng", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("eng", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("eng", "out", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        let grant = store
            .mint_overlay_grant("eng", "plan.md", "keeper", None)
            .await
            .unwrap();

        store.rename_domain("eng", "platform").await.unwrap();

        assert!(store.domain_visibility("eng").await.unwrap().is_none());
        assert!(store.domain_members("eng").await.unwrap().is_empty());
        assert_eq!(
            store
                .domain_visibility("platform")
                .await
                .unwrap()
                .map(|acl| acl.owner),
            Some("keeper".to_string())
        );
        let mut members: Vec<(String, MemberLevel)> = store
            .domain_members("platform")
            .await
            .unwrap()
            .into_iter()
            .map(|m| (m.principal, m.level))
            .collect();
        members.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            members,
            vec![
                ("mem".to_string(), MemberLevel::Viewer),
                ("out".to_string(), MemberLevel::Editor),
            ]
        );
        assert_eq!(
            store.overlay_grant_domain(&grant.token).await.unwrap(),
            Some("platform".to_string())
        );
        assert!(
            store
                .overlay_grants_of("keeper", "eng", "plan.md")
                .await
                .unwrap()
                .is_empty()
        );

        store
            .rename_domain("eng", "platform")
            .await
            .expect("a second call finds nothing under the old name");
        assert_eq!(store.domain_members("platform").await.unwrap().len(), 2);
        store
            .rename_domain("never", "seen")
            .await
            .expect("a domain with no records has nothing to move");
    }

    /// The renamed domain's records win over stale ones already under the new
    /// name: the stale acl owner, every stale member (colliding or not) and
    /// every stale share-link go, so none of them gains access to the renamed
    /// domain. A second call leaves the moved rows alone.
    #[tokio::test]
    async fn renaming_onto_stale_records_keeps_the_renamed_domains_rows() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("eng", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("eng", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        let live = store
            .mint_overlay_grant("eng", "plan.md", "keeper", None)
            .await
            .unwrap();
        // Stale records under the new name, from a removal that never forgot
        // them: another owner, a colliding and a non-colliding member, and a
        // share-link.
        store
            .set_domain_visibility("platform", true, "out")
            .await
            .unwrap();
        store
            .upsert_domain_member("platform", "mem", MemberLevel::Viewer, None, "out")
            .await
            .unwrap();
        store
            .upsert_domain_member("platform", "keeper", MemberLevel::Manager, None, "out")
            .await
            .unwrap();
        let stale = store
            .mint_overlay_grant("platform", "old.md", "out", None)
            .await
            .unwrap();

        store.rename_domain("eng", "platform").await.unwrap();
        assert!(store.domain_visibility("eng").await.unwrap().is_none());
        assert!(store.domain_members("eng").await.unwrap().is_empty());
        renamed_records_won(&store, &live.token, &stale.token).await;

        store
            .rename_domain("eng", "platform")
            .await
            .expect("a second call finds nothing under the old name");
        renamed_records_won(&store, &live.token, &stale.token).await;
    }

    /// What `renaming_onto_stale_records_keeps_the_renamed_domains_rows`
    /// expects under `platform` after each call.
    async fn renamed_records_won(store: &AuthStore, live: &str, stale: &str) {
        assert_eq!(
            store
                .domain_visibility("platform")
                .await
                .unwrap()
                .map(|acl| acl.owner),
            Some("keeper".to_string()),
            "the renamed domain's owner, not the stale one"
        );
        let members: Vec<(String, MemberLevel)> = store
            .domain_members("platform")
            .await
            .unwrap()
            .into_iter()
            .map(|m| (m.principal, m.level))
            .collect();
        assert_eq!(
            members,
            vec![("mem".to_string(), MemberLevel::Editor)],
            "only the renamed domain's members, at their levels"
        );
        assert_eq!(
            store.overlay_grant_domain(live).await.unwrap(),
            Some("platform".to_string())
        );
        assert_eq!(
            store.overlay_grant_domain(stale).await.unwrap(),
            None,
            "the stale share-link is gone"
        );
    }

    /// A rename that changes only the case is a real rename: names are stored
    /// as registered and compare byte for byte.
    #[tokio::test]
    async fn a_case_only_domain_rename_moves_the_records() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("Eng", true, "keeper")
            .await
            .unwrap();
        store.rename_domain("Eng", "eng").await.unwrap();
        assert!(store.domain_visibility("Eng").await.unwrap().is_none());
        assert!(store.domain_visibility("eng").await.unwrap().is_some());
    }

    /// Privatizing a domain that is already private writes nothing: not the
    /// owner, not the membership rows. It used to hand the domain to whoever
    /// asked last, which made a retried request an ownership transfer nobody
    /// asked for and dropped the previous owner - who holds no membership row
    /// by construction - to no access at all.
    #[tokio::test]
    async fn privatizing_an_already_private_domain_writes_nothing() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();

        let again = store
            .set_domain_visibility("lab", true, "out")
            .await
            .unwrap();
        assert_eq!(
            again,
            VisibilityWrite::AlreadyPrivate {
                owner: "keeper".to_string()
            },
            "the answer says it was already private, and names the owner it kept"
        );
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            "keeper",
            "a visibility statement is not an ownership transfer"
        );
        assert_eq!(
            store.domain_members("lab").await.unwrap().len(),
            1,
            "and nobody was evicted"
        );

        // The verb that DOES change an owner still does, and still keeps the
        // members: the two paths that change an owner must agree, and one of
        // them is no longer this one.
        store.transfer_domain("lab", "out").await.unwrap();
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            "out"
        );
        assert_eq!(
            store.domain_members("lab").await.unwrap().len(),
            1,
            "an owner change is not an eviction"
        );
    }

    // `promoting_a_member_to_owner_drops_the_row_that_now_says_less` used to sit
    // here, promoting a member by privatizing an already-private domain with a
    // new owner. That is not a thing this method does any more - privatizing
    // one that is already private writes nothing at all - and the step it was
    // really about is `transfer_domain`'s, which the test below pins on the one
    // path that still takes it.

    #[tokio::test]
    async fn transfer_domain_swaps_the_owner_and_drops_its_member_row() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "out", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        store.transfer_domain("lab", "mem").await.unwrap();
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            "mem"
        );
        let members = store.domain_members("lab").await.unwrap();
        assert_eq!(
            members
                .iter()
                .map(|m| m.principal.as_str())
                .collect::<Vec<_>>(),
            vec!["out"],
            "the new owner's membership row is gone: it could only say less"
        );
        assert!(store.memberships_of("mem").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transfer_domain_refuses_a_shared_domain_and_a_dead_owner() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        let err = store
            .transfer_domain("lab", "mem")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not private"), "{err}");
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let err = store
            .transfer_domain("lab", "ghost")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such user"), "{err}");
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            "keeper",
            "a refused transfer leaves the owner alone"
        );
    }

    #[tokio::test]
    async fn membership_is_upserted_and_removed_by_name() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "MEM", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        let members = store.domain_members("lab").await.unwrap();
        assert_eq!(
            members.len(),
            1,
            "an upsert replaces rather than duplicates"
        );
        assert_eq!(members[0].principal, "mem", "the name is folded");
        assert_eq!(members[0].level, MemberLevel::Manager);
        assert_eq!(members[0].added_by, "keeper");
        assert!(!members[0].added_at.is_empty());
        assert_eq!(
            store.memberships_of("Mem").await.unwrap(),
            vec![("lab".to_string(), MemberLevel::Manager)]
        );
        assert!(store.remove_domain_member("lab", "mem").await.unwrap());
        assert!(
            !store.remove_domain_member("lab", "mem").await.unwrap(),
            "removing what is not there reports itself rather than erroring"
        );
        assert!(store.domain_members("lab").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn membership_refuses_a_shared_domain_a_stranger_and_the_owner() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        let err = store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not private"), "{err}");
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let err = store
            .upsert_domain_member("lab", "ghost", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such user"), "{err}");
        let err = store
            .upsert_domain_member("lab", "keeper", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("already holds every level"), "{err}");
        store.set_disabled("mem", true).await.unwrap();
        let err = store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("disabled"), "{err}");
        let err = store
            .upsert_domain_member("lab", "out", MemberLevel::Viewer, None, "  ")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("an actor to record it as"), "{err}");
        assert!(store.domain_members("lab").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn removing_a_user_takes_its_memberships_with_it() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .add_user("boss", "boss", None, Role::Admin, "pw12345678")
            .await
            .unwrap();
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "out", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        store.remove_user("mem").await.unwrap();
        store.remove_user_force("out").await.unwrap();
        assert!(
            store.domain_members("lab").await.unwrap().is_empty(),
            "a re-added name must not inherit the old holder's access"
        );
    }

    #[tokio::test]
    async fn removing_a_user_leaves_the_domains_it_owned_owned_by_nobody() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        store.remove_user("keeper").await.unwrap();
        let acl = store
            .domain_visibility("lab")
            .await
            .unwrap()
            .expect("the domain stays private when its owner goes");
        assert_eq!(
            acl.owner, "",
            "owned by nobody: a name no live account can ever hold"
        );
        assert_eq!(
            store.domain_members("lab").await.unwrap().len(),
            1,
            "and the people invited into it keep their levels"
        );
        // Re-adding the freed name mints a different person. The resolver half
        // of this is `a_re_added_owner_name_does_not_inherit_the_domain` in
        // `crate::scope`; here the record itself must not name them.
        store
            .add_user("keeper", "keeper", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            ""
        );
        assert!(store.memberships_of("keeper").await.unwrap().is_empty());
    }

    /// The forced removal takes the same step. `remove_user_force` runs the
    /// same four cleanups the guarded remove does, and this is the one of them
    /// that widens rather than narrows if it is ever dropped: a private domain
    /// left naming a departed account would hand it to whoever next signs up
    /// under that login name. The guarded path is pinned above; a `--force`
    /// removal is exactly the path an operator reaches for when the account
    /// being removed is the last admin, so it must not be the one that leaks.
    #[tokio::test]
    async fn force_removing_a_user_also_leaves_its_domains_owned_by_nobody() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
        store.remove_user_force("keeper").await.unwrap();
        let acl = store
            .domain_visibility("lab")
            .await
            .unwrap()
            .expect("the domain stays private when its owner is forced out");
        assert_eq!(
            acl.owner, "",
            "owned by nobody, exactly as the guarded removal leaves it"
        );
        assert_eq!(
            store.domain_members("lab").await.unwrap().len(),
            1,
            "and the people invited into it keep their levels"
        );
    }

    /// Every membership refusal carries its kind as a TYPE, and its own
    /// sentence as the message.
    ///
    /// The pin that lets `rest::members` classify by value: matching prose
    /// meant a rewording here silently turned a 409 into a 500. Both halves
    /// are asserted - the kind, which the surface decides on, and the
    /// message, which the operator-facing surfaces still print - so neither
    /// can be dropped in favour of the other.
    #[tokio::test]
    async fn every_membership_refusal_carries_its_kind_and_its_words() {
        let (_dir, store) = store().await;
        members_cast(&store).await;

        let shared = store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&shared),
            Some(RefusalKind::NotPrivate)
        );
        assert!(
            format!("{shared:#}").contains("is not private"),
            "{shared:#}"
        );

        let no_owner = store.transfer_domain("lab", "mem").await.unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&no_owner),
            Some(RefusalKind::NotPrivate)
        );

        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        let owner = store
            .upsert_domain_member("lab", "keeper", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&owner),
            Some(RefusalKind::OwnerIsNotAMember)
        );
        assert!(format!("{owner:#}").contains("owns domain"), "{owner:#}");

        let ghost = store
            .upsert_domain_member("lab", "ghost", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&ghost),
            Some(RefusalKind::NoSuchAccount)
        );

        // A disabled account is the SAME kind as one that does not exist. The
        // two are told apart only in the message, which is what lets a surface
        // that must not distinguish them answer with one word-for-word reply
        // and be sure it has no second branch.
        store.set_disabled("mem", true).await.unwrap();
        let disabled = store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&disabled),
            Some(RefusalKind::NoSuchAccount),
            "an existing-but-disabled account is not a kind of its own"
        );
        assert!(
            format!("{disabled:#}").contains("is disabled"),
            "{disabled:#}"
        );

        // A principal that cannot be a login name at all carries its own kind,
        // which is what lets the one route taking a principal in the BODY
        // answer the 422 its documentation promises rather than a 500.
        let malformed = store
            .upsert_domain_member("lab", "  ", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&malformed),
            Some(RefusalKind::InvalidName)
        );

        // A domain with no acl row is shared as far as this table knows, so it
        // refuses as NotPrivate rather than as a fault. Named for what it
        // asserts: this leg is not the no-kind one.
        let unregistered = store
            .upsert_domain_member("nosuchdomain", "mem", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&unregistered),
            Some(RefusalKind::NotPrivate),
            "a domain with no acl row is shared as far as this table knows"
        );

        // And a failure that is nobody's doing carries no kind at all, so the
        // surfaces keep answering 500 for what is genuinely theirs. This is the
        // arm `rest::members::store_error`'s `None` branch answers, and without
        // a leg here nothing in the workspace pins it.
        let no_actor = store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "  ")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&no_actor),
            None,
            "an empty actor is this server's own bug, not a refusal with a kind"
        );
        assert!(
            format!("{no_actor:#}").contains("needs an actor"),
            "{no_actor:#}"
        );
    }

    #[tokio::test]
    async fn the_membership_tables_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        {
            let store = AuthStore::open(&path).await.unwrap();
            members_cast(&store).await;
            store
                .set_domain_visibility("lab", true, "keeper")
                .await
                .unwrap();
            store
                .upsert_domain_member("lab", "mem", MemberLevel::Manager, None, "keeper")
                .await
                .unwrap();
        }
        // The schema is idempotent DDL, so opening the same file again applies
        // it a second time and must change nothing.
        let store = AuthStore::open(&path).await.unwrap();
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            "keeper"
        );
        assert_eq!(
            store.memberships_of("mem").await.unwrap(),
            vec![("lab".to_string(), MemberLevel::Manager)]
        );
    }

    #[test]
    fn member_levels_round_trip_through_text_and_json() {
        for level in [
            MemberLevel::Viewer,
            MemberLevel::Editor,
            MemberLevel::Manager,
        ] {
            assert_eq!(level.to_string().parse::<MemberLevel>().unwrap(), level);
            assert_eq!(
                serde_json::to_string(&level).unwrap(),
                format!("\"{}\"", level.as_str())
            );
        }
        assert_eq!(
            " Manager ".parse::<MemberLevel>().unwrap(),
            MemberLevel::Manager
        );
        assert!("keeper".parse::<MemberLevel>().is_err());
        assert_eq!(
            member_level_from_db("nonsense"),
            MemberLevel::Viewer,
            "an unreadable row never fails open"
        );
    }

    #[test]
    fn a_domain_name_is_trimmed_and_never_empty() {
        assert_eq!(normalize_domain("  lab ").unwrap(), "lab");
        assert_eq!(normalize_domain("My Domain").unwrap(), "My Domain");
        assert!(normalize_domain("   ").is_err());
    }

    // --- identity links ---------------------------------------------------

    /// An account provisioned from an identity is found back by that
    /// identity, and by nothing else.
    #[tokio::test]
    async fn a_provisioned_identity_is_found_back_by_its_pair() {
        let (_dir, store) = store().await;
        let user = store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "ada",
                Some("Ada Lovelace"),
                Some("ada@example.test"),
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        assert_eq!(user.name, "ada");
        assert_eq!(user.display, "Ada Lovelace");
        assert_eq!(user.email.as_deref(), Some("ada@example.test"));
        assert_eq!(user.role, Role::Viewer);

        let found = store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .expect("the pair names the account");
        assert_eq!(found.name, "ada");
        assert!(
            store
                .linked_user("https://idp.example", "sub-2")
                .await
                .unwrap()
                .is_none(),
            "another subject at the same issuer is another person"
        );
        assert!(
            store
                .linked_user("https://other.example", "sub-1")
                .await
                .unwrap()
                .is_none(),
            "the same subject at another issuer is another person"
        );
        let links = store.identity_links("ada").await.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].issuer, "https://idp.example");
        assert_eq!(links[0].subject, "sub-1");
        assert_eq!(links[0].linked_by, "jit");
        assert!(!links[0].linked_at.is_empty());
    }

    /// A link whose account is gone is swept the moment it is looked up,
    /// rather than left dormant for the next account to take the name.
    ///
    /// The direct analogue of
    /// `an_orphaned_session_row_is_swept_rather_than_inherited`, and for the
    /// same reason: an identity link is a credential, and a dormant one signs
    /// a stranger into whoever next holds that login name.
    #[tokio::test]
    async fn an_orphaned_identity_link_is_swept_rather_than_inherited() {
        let (_dir, store) = store().await;
        store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "ada",
                Some("Ada"),
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        // Delete only the account row. Both removal paths are transactional
        // and sweep the links themselves, so this is the state a differently
        // built binary writing the same file, a hand edit, or a future
        // deletion path that forgets the sweep would leave behind.
        store
            .conn
            .execute(
                "DELETE FROM users WHERE name = ?1",
                vec![Value::Text("ada".to_string())],
            )
            .await
            .unwrap();
        assert!(
            store
                .linked_user("https://idp.example", "sub-1")
                .await
                .unwrap()
                .is_none()
        );
        store
            .add_user("ada", "Ada The Second", None, Role::Admin, "pw")
            .await
            .unwrap();
        assert!(
            store
                .linked_user("https://idp.example", "sub-1")
                .await
                .unwrap()
                .is_none(),
            "the orphan must have been swept, not left dormant"
        );
        assert!(
            store.identity_links("ada").await.unwrap().is_empty(),
            "and the account that took the name inherits nothing"
        );
    }

    /// The engine enforces both constraints, not only `insert_link`'s checks:
    /// a raw duplicate pair and a second identity for one account at one
    /// issuer are both refused by the database itself.
    #[tokio::test]
    async fn the_database_itself_refuses_a_duplicate_pair_and_a_second_identity() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
        let insert = |subject: &str, user: &str| {
            store.conn.execute(
                "INSERT INTO identity_link (issuer, subject, user, linked_at, linked_by)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                vec![
                    Value::Text("https://idp.example".to_string()),
                    Value::Text(subject.to_string()),
                    Value::Text(user.to_string()),
                    Value::Text("2026-09-07T00:00:00Z".to_string()),
                    Value::Text("test".to_string()),
                ],
            )
        };
        insert("sub-1", "ada").await.unwrap();
        assert!(
            insert("sub-1", "ada").await.is_err(),
            "the primary key on (issuer, subject) is the engine's, not ours"
        );
        assert!(
            insert("sub-2", "ada").await.is_err(),
            "and one account holds one identity per issuer, by unique index"
        );
    }

    /// A name already taken is uniquified rather than joined, and the
    /// suffixes keep counting.
    #[tokio::test]
    async fn provisioning_uniquifies_a_taken_name() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        let second = store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "Ada",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        assert_eq!(second.name, "ada-2");
        let third = store
            .provision_linked_user(
                "https://idp.example",
                "sub-2",
                "ada",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        assert_eq!(third.name, "ada-3");
        assert_eq!(
            store.user("ada").await.unwrap().unwrap().role,
            Role::Admin,
            "the account that held the name is untouched"
        );
    }

    /// Provisioning honours the account cap, and refuses in words that name
    /// the setting.
    #[tokio::test]
    async fn provisioning_refuses_past_the_account_cap() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        let err = store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "grace",
                None,
                None,
                Role::Viewer,
                1,
            )
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("auth.max_users"), "{message}");
        assert!(store.user("grace").await.unwrap().is_none());
        assert!(
            store
                .linked_user("https://idp.example", "sub-1")
                .await
                .unwrap()
                .is_none(),
            "a refused provisioning leaves no link behind"
        );
    }

    /// A pair belongs to one account, and an account holds one identity per
    /// issuer. Both refusals name what is in the way.
    #[tokio::test]
    async fn a_pair_links_once_and_an_account_holds_one_identity_per_issuer() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        store
            .add_user("grace", "Grace", None, Role::Editor, "correct horse")
            .await
            .unwrap();
        store
            .link_identity("https://idp.example", "sub-1", "ada", "ada")
            .await
            .unwrap();

        let err = store
            .link_identity("https://idp.example", "sub-1", "grace", "grace")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("already linked"), "{err:#}");

        let err = store
            .link_identity("https://idp.example", "sub-2", "ada", "ada")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("already holds"), "{err:#}");

        // A second issuer is a second identity, and that is allowed.
        store
            .link_identity("https://other.example", "sub-2", "ada", "ada")
            .await
            .unwrap();
        assert_eq!(store.identity_links("ada").await.unwrap().len(), 2);
        assert!(store.identity_links("grace").await.unwrap().is_empty());
    }

    /// Linking refuses a name that is nobody, so a link can never point at an
    /// account that does not exist.
    #[tokio::test]
    async fn an_identity_cannot_be_linked_to_a_name_that_is_nobody() {
        let (_dir, store) = store().await;
        let err = store
            .link_identity("https://idp.example", "sub-1", "ghost", "admin")
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no such user"), "{err:#}");
    }

    /// Unlinking removes the link and says whether there was one.
    #[tokio::test]
    async fn unlinking_reports_whether_it_removed_anything() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        store
            .link_identity("https://idp.example", "sub-1", "ada", "ada")
            .await
            .unwrap();
        assert!(
            store
                .unlink_identity("https://idp.example", "ada")
                .await
                .unwrap()
        );
        assert!(
            !store
                .unlink_identity("https://idp.example", "ada")
                .await
                .unwrap(),
            "the second unlink had nothing to remove"
        );
        assert!(
            store
                .linked_user("https://idp.example", "sub-1")
                .await
                .unwrap()
                .is_none()
        );
    }

    /// An account provisioned by a sign-on has no password, so its identity
    /// link is the only way into it: unlinking the last one is refused, in
    /// words that name the command which gives it a second way in.
    #[tokio::test]
    async fn unlinking_the_last_way_into_an_account_is_refused() {
        let (_dir, store) = store().await;
        store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "ada",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        let err = store
            .unlink_identity("https://idp.example", "ada")
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(
            message.contains("crystalline users passwd"),
            "the refusal teaches the way out: {message}"
        );
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::LastCredential),
            "the refusal travels as a type, not as prose"
        );
        assert!(
            store
                .linked_user("https://idp.example", "sub-1")
                .await
                .unwrap()
                .is_some(),
            "a refused unlink removes nothing"
        );
    }

    /// The rule is about the last way IN, not about the last link: an account
    /// with a password may unlink everything, and an account with two
    /// identities may unlink one of them.
    #[tokio::test]
    async fn a_second_way_in_is_what_makes_an_unlink_allowed() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        store
            .link_identity("https://idp.example", "sub-1", "ada", "ada")
            .await
            .unwrap();
        assert!(
            store
                .unlink_identity("https://idp.example", "ada")
                .await
                .unwrap(),
            "a password is the other way in"
        );

        store
            .provision_linked_user(
                "https://idp.example",
                "sub-2",
                "grace",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        store
            .link_identity("https://other.example", "sub-9", "grace", "grace")
            .await
            .unwrap();
        assert!(
            store
                .unlink_identity("https://idp.example", "grace")
                .await
                .unwrap(),
            "the other identity is the other way in"
        );
        let err = store
            .unlink_identity("https://other.example", "grace")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::LastCredential),
            "the one that is left is the last way in"
        );
    }

    /// An unlink that removes nothing is not a refusal: the account keeps
    /// whatever it had, so there is nothing to protect it from.
    #[tokio::test]
    async fn an_unlink_at_an_issuer_holding_no_link_is_a_no_op() {
        let (_dir, store) = store().await;
        store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "ada",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        assert!(
            !store
                .unlink_identity("https://elsewhere.example", "ada")
                .await
                .unwrap(),
            "there was no link at that issuer to remove"
        );
    }

    /// The admin repair: an operator whose provider re-issued its subjects
    /// unlinks the stale identity and links the new one, which needs a way
    /// past the guard. Forcing it is the only way, and it does strand the
    /// account until the link is remade.
    #[tokio::test]
    async fn a_forced_unlink_is_the_admin_repair() {
        let (_dir, store) = store().await;
        store
            .provision_linked_user(
                "https://idp.example",
                "old-sub",
                "ada",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        assert!(
            store
                .unlink_identity_force("https://idp.example", "ada")
                .await
                .unwrap()
        );
        assert!(store.identity_links("ada").await.unwrap().is_empty());
        store
            .link_identity("https://idp.example", "new-sub", "ada", LINKED_BY_CLI)
            .await
            .unwrap();
        let found = store
            .linked_user("https://idp.example", "new-sub")
            .await
            .unwrap()
            .expect("the repaired pair names the account");
        assert_eq!(found.name, "ada");
        assert_eq!(
            store.identity_links("ada").await.unwrap()[0].linked_by,
            LINKED_BY_CLI
        );
    }

    /// Whether an account has a local password, which is what the unlink rule
    /// and the profile card both turn on. A name that is nobody has none.
    #[tokio::test]
    async fn a_password_is_reported_without_being_read_back() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "grace",
                None,
                None,
                Role::Viewer,
                100,
            )
            .await
            .unwrap();
        assert!(store.has_password("ada").await.unwrap());
        assert!(!store.has_password("grace").await.unwrap());
        assert!(!store.has_password("nobody").await.unwrap());
        store.set_password("grace", "correct horse").await.unwrap();
        assert!(
            store.has_password("grace").await.unwrap(),
            "`users passwd` is what gives a provisioned account a second way in"
        );
    }

    /// Removing an account takes its identity links with it, so the pair
    /// cannot resurrect a deleted account on the next sign-in.
    #[tokio::test]
    async fn removing_an_account_takes_its_identity_links() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        store
            .add_user("grace", "Grace", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        store
            .link_identity("https://idp.example", "sub-1", "ada", "ada")
            .await
            .unwrap();
        store
            .link_identity("https://idp.example", "sub-2", "grace", "grace")
            .await
            .unwrap();

        store.remove_user("ada").await.unwrap();
        assert!(
            store
                .linked_user("https://idp.example", "sub-1")
                .await
                .unwrap()
                .is_none()
        );
        store.remove_user_force("grace").await.unwrap();
        assert!(
            store
                .linked_user("https://idp.example", "sub-2")
                .await
                .unwrap()
                .is_none()
        );
    }

    /// Presentation data refreshes, and an absent claim clears nothing.
    #[tokio::test]
    async fn refreshing_presentation_never_clears_what_was_not_sent() {
        let (_dir, store) = store().await;
        store
            .add_user(
                "ada",
                "Ada",
                Some("ada@example.test"),
                Role::Editor,
                "correct horse",
            )
            .await
            .unwrap();
        let refreshed = store
            .refresh_presentation("ada", Some("Ada L"), Some("new@example.test"))
            .await
            .unwrap();
        assert_eq!(refreshed.display, "Ada L");
        assert_eq!(refreshed.email.as_deref(), Some("new@example.test"));
        assert_eq!(refreshed.role, Role::Editor, "presentation moves no role");

        let untouched = store.refresh_presentation("ada", None, None).await.unwrap();
        assert_eq!(untouched.display, "Ada L");
        assert_eq!(untouched.email.as_deref(), Some("new@example.test"));
    }

    /// Two first sign-ins for one subject leave one account and one link.
    ///
    /// The property the single transaction buys: the loser's account insert
    /// rolls back with its refused link, rather than staying behind as an
    /// orphan nobody can sign into. An implementation that created the account
    /// and linked it in two calls would pass every other test here and leave
    /// `ada-2` behind on this one.
    #[tokio::test]
    async fn two_first_sign_ins_for_one_subject_leave_one_account_and_one_link() {
        let (_dir, store) = store().await;
        let provision = || {
            store.provision_linked_user(
                "https://idp.example",
                "sub-1",
                "ada",
                Some("Ada"),
                None,
                Role::Viewer,
                100,
            )
        };
        let outcomes = {
            let (first, second) = tokio::join!(provision(), provision());
            [first, second]
        };
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "exactly one of the two races provisions"
        );
        let refusal = outcomes
            .iter()
            .find_map(|outcome| outcome.as_ref().err())
            .expect("the loser is refused");
        assert!(
            format!("{refusal:#}").contains("already linked"),
            "{refusal:#}"
        );
        assert_eq!(
            store.list_users().await.unwrap().len(),
            1,
            "the loser left no orphan account behind"
        );
        assert_eq!(store.identity_links("ada").await.unwrap().len(), 1);
    }

    /// The resource every grant below is issued for: the origin the MCP
    /// transport is served from, no trailing slash, which is the spelling
    /// [`normalize_resource`] settles on.
    const OAUTH_RESOURCE: &str = "https://crystal.example";

    /// The OAuth cast every grant test starts from: one account and one
    /// registered client.
    async fn oauth_cast(store: &AuthStore) -> OauthClient {
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        store
            .register_oauth_client(
                "Some Client",
                Some("https://client.example"),
                &["https://client.example/callback".to_string()],
            )
            .await
            .unwrap()
    }

    /// One integer column of one grant row, for the assertions that are about
    /// what the database holds rather than about what a method answered.
    async fn grant_int(store: &AuthStore, id: i64, column: &str) -> i64 {
        let row = store
            .query_first(
                &format!("SELECT {column} FROM oauth_grants WHERE id = ?1"),
                vec![Value::Integer(id)],
            )
            .await
            .unwrap()
            .expect("the grant row is there");
        match row.get_value(0) {
            Ok(Value::Integer(n)) => n,
            other => panic!("unexpected {column}: {other:?}"),
        }
    }

    /// The same for a text column.
    async fn grant_text(store: &AuthStore, id: i64, column: &str) -> String {
        let row = store
            .query_first(
                &format!("SELECT {column} FROM oauth_grants WHERE id = ?1"),
                vec![Value::Integer(id)],
            )
            .await
            .unwrap()
            .expect("the grant row is there");
        cell_text(&row, 0).unwrap_or_default()
    }

    /// One text column of the token row a refresh token is stored under.
    async fn token_text(store: &AuthStore, refresh_token: &str, column: &str) -> Option<String> {
        let row = store
            .query_first(
                &format!("SELECT {column} FROM oauth_grant_token WHERE refresh_hash = ?1"),
                vec![Value::Text(token_hash(refresh_token))],
            )
            .await
            .unwrap()
            .expect("the token row is there");
        cell_text(&row, 0)
    }

    /// The same for an integer column, `None` for NULL.
    async fn token_int(store: &AuthStore, refresh_token: &str, column: &str) -> Option<i64> {
        let row = store
            .query_first(
                &format!("SELECT {column} FROM oauth_grant_token WHERE refresh_hash = ?1"),
                vec![Value::Text(token_hash(refresh_token))],
            )
            .await
            .unwrap()
            .expect("the token row is there");
        match row.get_value(0) {
            Ok(Value::Integer(n)) => Some(n),
            _ => None,
        }
    }

    async fn count_rows(store: &AuthStore, table: &str) -> i64 {
        let row = store
            .query_first(&format!("SELECT COUNT(*) FROM {table}"), vec![])
            .await
            .unwrap()
            .expect("COUNT(*) always answers");
        match row.get_value(0) {
            Ok(Value::Integer(n)) => n,
            other => panic!("unexpected COUNT(*) result: {other:?}"),
        }
    }

    /// The audience rule and the expiry, the two properties an access token
    /// carries beyond "this is ada". The expiry is reached by winding the
    /// column back through the connection, because `issue_oauth_grant` takes
    /// no TTL: the thirty days and the hour are the store's, not a caller's.
    #[tokio::test]
    async fn an_oauth_grant_resolves_only_for_its_resource_and_until_it_expires() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        assert!(issued.access_token.starts_with(OAUTH_ACCESS_PREFIX));
        assert!(issued.refresh_token.starts_with(OAUTH_REFRESH_PREFIX));
        assert_eq!(issued.expires_in, OAUTH_ACCESS_TTL_SECS as u64);
        assert_eq!(
            token_text(&store, &issued.refresh_token, "access_hash").await,
            Some(token_hash(&issued.access_token)),
            "only the hash is written, the same way an MCP token is stored"
        );
        assert!(
            grant_text(&store, issued.id, "access_hash")
                .await
                .starts_with(OAUTH_PLACEHOLDER_PREFIX),
            "the grant row's own token columns hold only placeholders"
        );

        let user = store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .expect("the token resolves for the resource it was issued for");
        assert_eq!(user.name, "ada");
        assert!(
            store
                .oauth_access_user(&issued.access_token, "https://crystal.example/")
                .await
                .unwrap()
                .is_some(),
            "one trailing slash is the same resource"
        );
        assert!(
            store
                .oauth_access_user(&issued.access_token, "https://other.example")
                .await
                .unwrap()
                .is_none(),
            "a token issued for one resource never opens another"
        );
        assert!(
            store
                .oauth_access_user("coa_not-a-token", OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none()
        );

        let listed = store.list_oauth_grants("ada").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].last_used.is_some(),
            "resolving stamped the grant, which is what the management list shows"
        );

        store.set_disabled("ada", true).await.unwrap();
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "a disabled account's grant resolves for nobody"
        );
        store.set_disabled("ada", false).await.unwrap();
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some(),
            "and re-enabling hands it back, the way a disabled account's MCP tokens come back"
        );

        store
            .conn
            .execute(
                "UPDATE oauth_grant_token SET access_expires_at = 1 WHERE grant_id = ?1",
                vec![Value::Integer(issued.id)],
            )
            .await
            .unwrap();
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "an expired access token resolves for nobody"
        );
    }

    /// A grant is a row about an account and a registration, so neither may be
    /// invented by the write. The transaction is what makes the two checks
    /// mean anything at the moment of the insert.
    #[tokio::test]
    async fn a_grant_needs_a_live_account_and_a_registered_client() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let ghost = store
            .issue_oauth_grant("ghost", &client.client_id, OAUTH_RESOURCE)
            .await
            .expect_err("an account that is nobody gets no grant");
        assert!(format!("{ghost:#}").contains("no such user"), "{ghost:#}");

        let stranger = store
            .issue_oauth_grant("ada", "coc_nobody", OAUTH_RESOURCE)
            .await
            .expect_err("a registration that does not exist gets no grant");
        assert!(
            format!("{stranger:#}").contains("no such oauth client"),
            "{stranger:#}"
        );

        store.set_disabled("ada", true).await.unwrap();
        let disabled = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .expect_err("a disabled account gets no new grant");
        assert!(format!("{disabled:#}").contains("disabled"), "{disabled:#}");
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    }

    /// What a rotation is: one grant row moved along, both tokens replaced,
    /// the predecessor's access token dead on the spot and the refresh window
    /// restarted. Presenting the predecessor's *refresh* token is a replay
    /// rather than a miss, so that half is
    /// [`a_replayed_refresh_token_revokes_the_whole_grant`]'s.
    #[tokio::test]
    async fn a_refresh_rotates_both_tokens_and_the_predecessor_stops_working() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let first = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        // Bring the refresh window in close, so "the thirty days start at
        // rotation" cannot pass by accident on the window the issue opened.
        store
            .conn
            .execute(
                "UPDATE oauth_grants SET refresh_expires_at = ?2 WHERE id = ?1",
                vec![
                    Value::Integer(first.id),
                    Value::Integer(chrono::Utc::now().timestamp() + 60),
                ],
            )
            .await
            .unwrap();

        let outcome = store
            .refresh_oauth_grant(&first.refresh_token, &client.client_id, None)
            .await
            .unwrap();
        let RefreshOutcome::Rotated(second) = outcome else {
            panic!("a live refresh token rotates, got {outcome:?}");
        };
        assert_eq!(
            second.id, first.id,
            "a rotation moves one grant along rather than forking it"
        );
        assert_ne!(second.access_token, first.access_token);
        assert_ne!(second.refresh_token, first.refresh_token);
        assert_eq!(second.expires_in, OAUTH_ACCESS_TTL_SECS as u64);
        assert!(
            store
                .oauth_access_user(&first.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "the predecessor's access token stops the moment its successor exists"
        );
        assert!(
            store
                .oauth_access_user(&second.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            grant_int(&store, first.id, "refresh_expires_at").await
                > chrono::Utc::now().timestamp() + OAUTH_REFRESH_TTL_SECS - 60,
            "the new refresh token's thirty days start at the rotation"
        );
        assert!(
            token_int(&store, &first.refresh_token, "rotated_at")
                .await
                .is_some(),
            "a rotated row is kept with its rotation time, which is what makes a replay detectable"
        );
        assert!(
            token_text(&store, &first.refresh_token, "access_hash")
                .await
                .is_none(),
            "and its access token is gone"
        );
        assert_eq!(
            token_text(&store, &second.refresh_token, "rotated_from").await,
            Some(token_hash(&first.refresh_token)),
            "the successor names what it was minted from"
        );
        assert_eq!(store.list_oauth_grants("ada").await.unwrap().len(), 1);
    }

    /// A rotated refresh token coming back is evidence that a copy of it
    /// leaked, so the grant it belonged to is revoked rather than refreshed -
    /// and it is evidence whoever presents it, which is why the rule keys on
    /// the token alone while a rotation keys on the token and its client.
    #[tokio::test]
    async fn a_replayed_refresh_token_revokes_the_whole_grant() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let stranger = store
            .register_oauth_client(
                "Another Client",
                None,
                &["https://x.example/cb".to_string()],
            )
            .await
            .unwrap();
        let first = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let outcome = store
            .refresh_oauth_grant(&first.refresh_token, &client.client_id, None)
            .await
            .unwrap();
        let RefreshOutcome::Rotated(second) = outcome else {
            panic!("a live refresh token rotates, got {outcome:?}");
        };

        let replay = store
            .refresh_oauth_grant(&first.refresh_token, &stranger.client_id, None)
            .await
            .unwrap();
        match replay {
            RefreshOutcome::Replayed { grant, .. } => assert_eq!(grant, first.id),
            other => panic!("a rotated refresh token is a replay, got {other:?}"),
        }
        assert!(
            store
                .oauth_access_user(&second.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "the revocation took the successor's access token with it"
        );
        assert!(
            matches!(
                store
                    .refresh_oauth_grant(&second.refresh_token, &client.client_id, None)
                    .await
                    .unwrap(),
                RefreshOutcome::Unknown
            ),
            "and its refresh token, because the whole row is gone"
        );
        assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);
        let ended = store.list_ended_oauth_grants("ada").await.unwrap();
        assert_eq!(ended.len(), 1, "the replay is recorded");
        assert_eq!(
            ended[0].ended.as_ref().map(|e| e.reason),
            Some(OauthGrantEndReason::Replayed)
        );
    }

    /// A token two rotations back is a replay, even inside the grace window.
    /// The token it was rotated into has itself been rotated, so whoever
    /// held the answer has moved on and does not need the old token any
    /// more: its return is evidence of a copy, not of a parallel session.
    #[tokio::test]
    async fn a_token_two_rotations_back_is_a_replay() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let now = chrono::Utc::now().timestamp();
        let first = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let outcome = store
            .refresh_oauth_grant_at(&first.refresh_token, &client.client_id, None, now)
            .await
            .unwrap();
        let RefreshOutcome::Rotated(second) = outcome else {
            panic!("a live refresh token rotates, got {outcome:?}");
        };
        let outcome = store
            .refresh_oauth_grant_at(&second.refresh_token, &client.client_id, None, now + 1)
            .await
            .unwrap();
        let RefreshOutcome::Rotated(third) = outcome else {
            panic!("the successor rotates in its turn, got {outcome:?}");
        };
        assert!(
            store
                .oauth_access_user(&third.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some()
        );

        match store
            .refresh_oauth_grant_at(&first.refresh_token, &client.client_id, None, now + 2)
            .await
            .unwrap()
        {
            RefreshOutcome::Replayed {
                grant,
                since_rotation,
            } => {
                assert_eq!(grant, first.id);
                assert_eq!(since_rotation, 2, "seconds since its own rotation");
            }
            other => panic!("the token two rotations back is a replay, got {other:?}"),
        }
        assert!(
            store
                .oauth_access_user(&third.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "the replay revoked the grant, the newest pair with it"
        );
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);
        assert_eq!(count_rows(&store, "oauth_grant_token").await, 0);
    }

    /// The three ways a refresh misses without being a replay. None of them
    /// may touch the grant: an unknown token is a stranger, a live token at
    /// the wrong registration is a client mixing up its credentials, and an
    /// expired one is a client that waited too long. Only a token that was
    /// actually rotated revokes anything.
    #[tokio::test]
    async fn an_unknown_refresh_token_is_neither_rotated_nor_a_replay() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let stranger = store
            .register_oauth_client(
                "Another Client",
                None,
                &["https://x.example/cb".to_string()],
            )
            .await
            .unwrap();
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();

        for (token, client_id, why) in [
            (
                "cor_nothing".to_string(),
                client.client_id.clone(),
                "a token nobody issued",
            ),
            (
                issued.refresh_token.clone(),
                stranger.client_id.clone(),
                "a live token at the wrong registration",
            ),
        ] {
            assert!(
                matches!(
                    store
                        .refresh_oauth_grant(&token, &client_id, None)
                        .await
                        .unwrap(),
                    RefreshOutcome::Unknown
                ),
                "{why} rotates nothing"
            );
        }

        store.set_disabled("ada", true).await.unwrap();
        assert!(
            matches!(
                store
                    .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
                    .await
                    .unwrap(),
                RefreshOutcome::Unknown
            ),
            "a disabled account refreshes nothing"
        );
        store.set_disabled("ada", false).await.unwrap();

        store
            .conn
            .execute(
                "UPDATE oauth_grants SET refresh_expires_at = 1 WHERE id = ?1",
                vec![Value::Integer(issued.id)],
            )
            .await
            .unwrap();
        assert!(
            matches!(
                store
                    .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
                    .await
                    .unwrap(),
                RefreshOutcome::Unknown
            ),
            "an expired refresh token rotates nothing"
        );
        assert_eq!(
            count_rows(&store, "oauth_grants").await,
            1,
            "and none of the four refusals deleted the grant"
        );
    }

    /// Revoking is one delete, so both of a grant's tokens stop together, and
    /// removing the account sweeps what is left - the resurrection hazard
    /// `mcp_tokens` documents, one table over: a grant row that outlived its
    /// account would be inherited by the next holder of the freed name.
    #[tokio::test]
    async fn revoking_a_grant_stops_both_tokens_and_removing_the_account_sweeps_them() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();

        let listed = store.list_oauth_grants("ada").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, issued.id);
        assert_eq!(listed[0].client_id, client.client_id);
        assert_eq!(listed[0].client_name, "Some Client");
        assert_eq!(
            listed[0].redirect_host, "client.example",
            "the card names the host the client redirects to"
        );
        assert!(listed[0].last_used.is_none());
        assert!(
            chrono::DateTime::parse_from_rfc3339(&listed[0].refresh_expires_at).is_ok(),
            "the expiry a person reads is RFC 3339: {}",
            listed[0].refresh_expires_at
        );

        store
            .add_user("bob", "Bob", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        assert!(
            !store.revoke_oauth_grant("bob", issued.id).await.unwrap(),
            "another account's revoke never reaches this grant"
        );
        assert!(store.revoke_oauth_grant("ada", issued.id).await.unwrap());
        assert!(
            !store.revoke_oauth_grant("ada", issued.id).await.unwrap(),
            "a second revoke removes nothing"
        );
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            matches!(
                store
                    .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
                    .await
                    .unwrap(),
                RefreshOutcome::Unknown
            ),
            "one deleted row stops both tokens at once"
        );

        let second = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        store.remove_user("ada").await.unwrap();
        assert!(
            store
                .oauth_access_user(&second.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            count_rows(&store, "oauth_grants").await,
            0,
            "the removal swept the row rather than leaving it for the next holder of the name"
        );
        assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());
    }

    /// The forced removal path sweeps the same rows the guarded one does. The
    /// two are separate statements that have to be kept in step, which is why
    /// every sweep on this file carries a test on both.
    #[tokio::test]
    async fn force_removing_an_account_sweeps_its_oauth_grants_too() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        store.remove_user_force("ada").await.unwrap();
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    }

    /// A registration is what a client gets instead of a secret, so it has to
    /// come back exactly as it was written, including the order of its
    /// redirect uris - the token endpoint compares against them.
    #[tokio::test]
    async fn a_registration_is_read_back_by_its_client_id_and_touched() {
        let (_dir, store) = store().await;
        assert_eq!(store.count_oauth_clients().await.unwrap(), 0);
        let uris = vec![
            "http://127.0.0.1:33418/callback".to_string(),
            "https://client.example/cb".to_string(),
        ];
        let client = store
            .register_oauth_client("Some Client", Some("https://client.example"), &uris)
            .await
            .unwrap();
        assert!(client.client_id.starts_with(OAUTH_CLIENT_PREFIX));
        assert_eq!(
            client.client_id.len(),
            OAUTH_CLIENT_PREFIX.len() + 32,
            "a client id is the prefix plus 32 hex characters"
        );
        assert!(client.last_used.is_none());

        let read = store
            .oauth_client(&client.client_id)
            .await
            .unwrap()
            .expect("the registration reads back by its id");
        assert_eq!(read.client_name, "Some Client");
        assert_eq!(read.client_uri.as_deref(), Some("https://client.example"));
        assert_eq!(read.redirect_uris, uris);
        assert_eq!(read.created_at, client.created_at);
        assert!(store.oauth_client("coc_nobody").await.unwrap().is_none());
        assert_eq!(store.count_oauth_clients().await.unwrap(), 1);

        store.touch_oauth_client(&client.client_id).await.unwrap();
        let touched = store
            .oauth_client(&client.client_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            touched.last_used.is_some(),
            "an authorization a person allowed stamps the registration"
        );
        store
            .touch_oauth_client("coc_nobody")
            .await
            .expect("touching a registration that is gone is a no-op, not an error");

        assert!(
            store
                .register_oauth_client("Some Client", None, &[])
                .await
                .is_err(),
            "a registration with nowhere to redirect is refused"
        );
        let second = store
            .register_oauth_client("Another", None, &["https://a.example/cb".to_string()])
            .await
            .unwrap();
        assert_ne!(second.client_id, client.client_id);
        assert_eq!(store.count_oauth_clients().await.unwrap(), 2);
    }

    /// Registrations arrive one per fresh connection and nobody deletes them,
    /// so the store prunes the ones that never became a connection. A
    /// registration somebody is still connected through is never pruned, and
    /// an authorization a person allowed buys another thirty days.
    #[tokio::test]
    async fn an_unused_registration_is_pruned_after_thirty_days() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let fresh = store
            .register_oauth_client("Fresh", None, &["https://fresh.example/cb".to_string()])
            .await
            .unwrap();
        let stale = store
            .register_oauth_client("Stale", None, &["https://stale.example/cb".to_string()])
            .await
            .unwrap();
        let used = store
            .register_oauth_client("Used", None, &["https://used.example/cb".to_string()])
            .await
            .unwrap();
        let issued = store
            .issue_oauth_grant("ada", &used.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();

        // Age two of the three through the connection: the thirty days are the
        // store's, and no method winds the clock back.
        let long_ago = "2020-01-01T00:00:00+00:00";
        for id in [&stale.client_id, &used.client_id] {
            store
                .conn
                .execute(
                    "UPDATE oauth_clients SET created_at = ?2, last_used = NULL
                     WHERE client_id = ?1",
                    vec![Value::Text(id.clone()), Value::Text(long_ago.to_string())],
                )
                .await
                .unwrap();
        }
        assert_eq!(store.count_oauth_clients().await.unwrap(), 3);
        assert_eq!(
            store.prune_oauth_clients().await.unwrap(),
            1,
            "only the old registration that never produced a grant goes"
        );
        assert!(
            store
                .oauth_client(&stale.client_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .oauth_client(&fresh.client_id)
                .await
                .unwrap()
                .is_some(),
            "a registration inside the window stays"
        );
        assert!(
            store.oauth_client(&used.client_id).await.unwrap().is_some(),
            "and so does an old one somebody is still connected through"
        );
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some(),
            "the prune left the live connection alone"
        );

        // An authorization somebody allowed stamps `last_used`, and that is
        // what keeps an old registration alive for another thirty days. Nothing
        // an unauthenticated request does reaches this column: see
        // `only_an_allowed_authorization_stamps_a_registration_as_used`.
        store
            .conn
            .execute(
                "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
                vec![
                    Value::Text(fresh.client_id.clone()),
                    Value::Text(long_ago.to_string()),
                ],
            )
            .await
            .unwrap();
        store.touch_oauth_client(&fresh.client_id).await.unwrap();
        assert_eq!(store.prune_oauth_clients().await.unwrap(), 0);
        assert_eq!(store.count_oauth_clients().await.unwrap(), 2);
    }

    /// **Reading a registration is not using it**: only an authorization that
    /// a person actually allowed moves a row off the one-hour clock.
    ///
    /// The distinction is the whole of why the short clock works. Everything an
    /// authorization *request* does to a registration is read it - look it up
    /// by the id the caller named, compare the redirect uri it presented - and
    /// that request carries no identity: the person has not signed in yet, and
    /// a caller that registered the client is authorizing against its own row,
    /// so every check on that leg passes by construction. If reading stamped
    /// `last_used`, one extra unauthenticated request per row would move it to
    /// the thirty-day branch, and the fill-and-hold the short clock exists to
    /// prevent would be back at the cost of one GET.
    ///
    /// So [`AuthStore::touch_oauth_client`] is called when consent is ALLOWED
    /// and when a grant is issued or refreshed, never on the authorize request,
    /// and nothing else in this file writes that column. This test is the
    /// contract in executable form for the two tasks that hold those call
    /// sites.
    #[tokio::test]
    async fn only_an_allowed_authorization_stamps_a_registration_as_used() {
        let (_dir, store) = store().await;
        let client = store
            .register_oauth_client("Asker", None, &["https://asker.example/cb".to_string()])
            .await
            .unwrap();
        assert!(client.last_used.is_none());

        // Everything the authorize leg does to the row before a person has
        // decided anything, twice over.
        for _ in 0..2 {
            let read = store
                .oauth_client(&client.client_id)
                .await
                .unwrap()
                .expect("the registration reads back");
            assert!(
                read.last_used.is_none(),
                "reading a registration never stamps it"
            );
        }

        // Two hours later that row is still on the short clock, because
        // nothing it has been through counts as an authorization.
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        store
            .conn
            .execute(
                "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
                vec![
                    Value::Text(client.client_id.clone()),
                    Value::Text(two_hours_ago.clone()),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            store.prune_oauth_clients().await.unwrap(),
            1,
            "an hour of being asked about is not an hour of being used"
        );

        // The same row, with the one thing that does count: a person allowed
        // it, which is what the consent endpoint stamps.
        let allowed = store
            .register_oauth_client("Allowed", None, &["https://allowed.example/cb".to_string()])
            .await
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
                vec![
                    Value::Text(allowed.client_id.clone()),
                    Value::Text(two_hours_ago),
                ],
            )
            .await
            .unwrap();
        store.touch_oauth_client(&allowed.client_id).await.unwrap();
        assert_eq!(store.prune_oauth_clients().await.unwrap(), 0);
        assert!(
            store
                .oauth_client(&allowed.client_id)
                .await
                .unwrap()
                .is_some(),
            "an allowed authorization buys the thirty days"
        );
    }

    /// **A registration nobody ever authorized expires within the hour**, and
    /// only one somebody did gets the thirty days.
    ///
    /// The two clocks exist because the two rows mean different things. A row
    /// that never reached the authorize endpoint is the residue of a client
    /// that registered and walked away - or of an anonymous caller filling the
    /// table, which is the only way the cap is ever reached - and keeping it
    /// for thirty days is what would make that filling stick. A row that HAS
    /// authorized is somebody's client between connections, and thirty idle
    /// days is the right patience for it.
    #[tokio::test]
    async fn a_registration_that_never_authorized_expires_within_the_hour() {
        let (_dir, store) = store().await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let never = store
            .register_oauth_client("Never", None, &["https://never.example/cb".to_string()])
            .await
            .unwrap();
        let authorized = store
            .register_oauth_client("Asked", None, &["https://asked.example/cb".to_string()])
            .await
            .unwrap();
        let connected = store
            .register_oauth_client("Live", None, &["https://live.example/cb".to_string()])
            .await
            .unwrap();
        store
            .issue_oauth_grant("ada", &connected.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let just_now = store
            .register_oauth_client("Fresh", None, &["https://fresh.example/cb".to_string()])
            .await
            .unwrap();

        // Two hours ago for all three of the old ones, and the one that
        // authorized did so half an hour ago: well past the hour, nowhere near
        // the thirty days.
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let half_hour_ago = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
        for id in [
            &never.client_id,
            &authorized.client_id,
            &connected.client_id,
        ] {
            store
                .conn
                .execute(
                    "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
                    vec![Value::Text(id.clone()), Value::Text(two_hours_ago.clone())],
                )
                .await
                .unwrap();
        }
        store
            .conn
            .execute(
                "UPDATE oauth_clients SET last_used = ?2 WHERE client_id = ?1",
                vec![
                    Value::Text(authorized.client_id.clone()),
                    Value::Text(half_hour_ago),
                ],
            )
            .await
            .unwrap();

        assert_eq!(
            store.prune_oauth_clients().await.unwrap(),
            1,
            "only the registration nobody authorized, holding nothing, goes"
        );
        assert!(
            store
                .oauth_client(&never.client_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .oauth_client(&authorized.client_id)
                .await
                .unwrap()
                .is_some(),
            "a registration somebody authorized gets the thirty days"
        );
        assert!(
            store
                .oauth_client(&connected.client_id)
                .await
                .unwrap()
                .is_some(),
            "and one somebody is connected through is never collected at all"
        );
        assert!(
            store
                .oauth_client(&just_now.client_id)
                .await
                .unwrap()
                .is_some(),
            "a registration made a moment ago has its hour"
        );

        // The same row, thirty idle days later.
        store
            .conn
            .execute(
                "UPDATE oauth_clients SET last_used = ?2 WHERE client_id = ?1",
                vec![
                    Value::Text(authorized.client_id.clone()),
                    Value::Text("2020-01-01T00:00:00+00:00".to_string()),
                ],
            )
            .await
            .unwrap();
        assert_eq!(store.prune_oauth_clients().await.unwrap(), 1);
        assert!(
            store
                .oauth_client(&authorized.client_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The two unhashed copies of a live credential must never be one
    /// `tracing::debug!` or one failed assertion away from a log file, and the
    /// enum that carries a rotation's answer must not undo that.
    #[tokio::test]
    async fn an_issued_oauth_grant_never_prints_its_secrets() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let text = format!("{issued:?}");
        for (prefix, token) in [
            (OAUTH_ACCESS_PREFIX, &issued.access_token),
            (OAUTH_REFRESH_PREFIX, &issued.refresh_token),
        ] {
            let secret = token
                .strip_prefix(prefix)
                .expect("a token carries its prefix");
            assert!(!text.contains(secret), "the secret is redacted: {text}");
        }
        assert!(text.contains("redacted"), "and says so: {text}");
        assert!(
            text.contains(&issued.id.to_string()),
            "while the id still prints: {text}"
        );

        let rotated = store
            .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
            .await
            .unwrap();
        let printed = format!("{rotated:?}");
        let RefreshOutcome::Rotated(next) = &rotated else {
            panic!("a live refresh token rotates, got {printed}");
        };
        let secret = next
            .access_token
            .strip_prefix(OAUTH_ACCESS_PREFIX)
            .expect("a token carries its prefix");
        assert!(
            !printed.contains(secret),
            "the outcome redacts what it wraps: {printed}"
        );
    }

    /// A grant past its refresh window can never work again: no rotation, no
    /// replay, and its access token died an hour into the thirty days. So it
    /// is neither a connection to show a person nor a row to keep - and while
    /// it is kept, it pins its registration against the client prune, which is
    /// what makes the ordering of the two prunes load bearing.
    #[tokio::test]
    async fn an_expired_grant_is_pruned_and_never_listed_as_a_connection() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let live = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let dead = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        close_refresh_window(&store, dead.id).await;

        let listed = store.list_oauth_grants("ada").await.unwrap();
        assert_eq!(
            listed.len(),
            1,
            "a grant that cannot work again is not a connection anybody can act on"
        );
        assert_eq!(listed[0].id, live.id);
        assert_eq!(store.prune_oauth_grants().await.unwrap(), 1);
        assert_eq!(count_rows(&store, "oauth_grants").await, 1);
        assert!(
            store
                .oauth_access_user(&live.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some(),
            "the prune left the live grant alone"
        );

        // A registration abandoned after one connection: its dead grant holds
        // it against the client prune until the grant prune runs first.
        let abandoned = store
            .register_oauth_client("Abandoned", None, &["https://gone.example/cb".to_string()])
            .await
            .unwrap();
        let stranded = store
            .issue_oauth_grant("ada", &abandoned.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        close_refresh_window(&store, stranded.id).await;
        // Age *both* registrations past the window, so what keeps the first
        // one is its live grant rather than its freshness - which is the
        // property the last assertion is about.
        for id in [&abandoned.client_id, &client.client_id] {
            store
                .conn
                .execute(
                    "UPDATE oauth_clients SET created_at = '2020-01-01T00:00:00+00:00'
                     WHERE client_id = ?1",
                    vec![Value::Text(id.clone())],
                )
                .await
                .unwrap();
        }

        assert_eq!(
            store.prune_oauth_clients().await.unwrap(),
            0,
            "while the dead grant is there it pins the registration"
        );
        assert_eq!(store.prune_oauth_grants().await.unwrap(), 1);
        assert_eq!(
            store.prune_oauth_clients().await.unwrap(),
            1,
            "and the registration goes in the same pass, once its grant has"
        );
        assert!(
            store
                .oauth_client(&client.client_id)
                .await
                .unwrap()
                .is_some(),
            "the registration somebody is still connected through stays, \
             old as it is, because its live grant is what holds it"
        );
    }

    /// Close a grant's refresh window through the connection, the only way to
    /// reach an expiry the store owns.
    async fn close_refresh_window(store: &AuthStore, id: i64) {
        store
            .conn
            .execute(
                "UPDATE oauth_grants SET refresh_expires_at = 1 WHERE id = ?1",
                vec![Value::Integer(id)],
            )
            .await
            .unwrap();
    }

    /// Two parallel sessions of one client refresh with the same token: the
    /// first rotates, the second arrives inside the grace window and gets a
    /// pair of its own. Both pairs work, both refresh tokens are live, and the
    /// grant survives. The write lock decides the order, so racing requests
    /// reach the same two answers.
    #[tokio::test]
    async fn two_parallel_refreshes_inside_the_window_both_succeed() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let now = chrono::Utc::now().timestamp();
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let (one, two) = tokio::join!(
            store.refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now),
            store.refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now),
        );
        let mut pairs = Vec::new();
        let mut reissued = 0;
        for outcome in [one.unwrap(), two.unwrap()] {
            match outcome {
                RefreshOutcome::Rotated(pair) => pairs.push(pair),
                RefreshOutcome::Reissued {
                    grant,
                    since_rotation,
                } => {
                    assert_eq!(since_rotation, 0);
                    reissued += 1;
                    pairs.push(grant);
                }
                other => panic!("both parallel refreshes succeed, got {other:?}"),
            }
        }
        assert_eq!(reissued, 1, "one rotates, the other is reissued");
        assert_ne!(pairs[0].refresh_token, pairs[1].refresh_token);
        assert_ne!(pairs[0].access_token, pairs[1].access_token);
        for pair in &pairs {
            assert_eq!(pair.id, issued.id, "one grant, not a fork");
            assert_eq!(pair.expires_in, OAUTH_ACCESS_TTL_SECS as u64);
            assert!(
                store
                    .oauth_access_user(&pair.access_token, OAUTH_RESOURCE)
                    .await
                    .unwrap()
                    .is_some(),
                "each session's access token works"
            );
        }
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "the rotated token's own access token stopped"
        );
        // Each new refresh token is live in its own right.
        for (i, pair) in pairs.iter().enumerate() {
            let outcome = store
                .refresh_oauth_grant_at(&pair.refresh_token, &client.client_id, None, now + 10)
                .await
                .unwrap();
            assert!(
                matches!(outcome, RefreshOutcome::Rotated(_)),
                "session {i} rotates its own token, got {outcome:?}"
            );
        }
        assert_eq!(store.list_oauth_grants("ada").await.unwrap().len(), 1);
        assert!(
            store
                .list_ended_oauth_grants("ada")
                .await
                .unwrap()
                .is_empty(),
            "nothing ended"
        );
    }

    /// The same token presented again 60 seconds or more after its rotation
    /// is a replay: the grant is revoked and the record says why. 59 seconds
    /// is still inside the window.
    #[tokio::test]
    async fn the_same_token_after_the_window_revokes_the_grant() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let now = chrono::Utc::now().timestamp();
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let outcome = store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
            .await
            .unwrap();
        let RefreshOutcome::Rotated(rotated) = outcome else {
            panic!("a live refresh token rotates, got {outcome:?}");
        };
        let outcome = store
            .refresh_oauth_grant_at(
                &issued.refresh_token,
                &client.client_id,
                None,
                now + OAUTH_REFRESH_GRACE_SECS - 1,
            )
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                RefreshOutcome::Reissued {
                    since_rotation: 59,
                    ..
                }
            ),
            "59 seconds is inside the window, got {outcome:?}"
        );
        match store
            .refresh_oauth_grant_at(
                &issued.refresh_token,
                &client.client_id,
                None,
                now + OAUTH_REFRESH_GRACE_SECS,
            )
            .await
            .unwrap()
        {
            RefreshOutcome::Replayed {
                grant,
                since_rotation,
            } => {
                assert_eq!(grant, issued.id);
                assert_eq!(since_rotation, OAUTH_REFRESH_GRACE_SECS);
            }
            other => panic!("60 seconds after the rotation is a replay, got {other:?}"),
        }
        assert!(
            store
                .oauth_access_user(&rotated.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none(),
            "the whole grant is gone"
        );
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);
        assert_eq!(count_rows(&store, "oauth_grant_token").await, 0);
        let ended = store.list_ended_oauth_grants("ada").await.unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].id, issued.id);
        assert_eq!(ended[0].client_name, "Some Client");
        assert_eq!(ended[0].redirect_host, "client.example");
        assert_eq!(
            ended[0].ended.as_ref().map(|e| e.reason),
            Some(OauthGrantEndReason::Replayed)
        );
    }

    /// Inside the window, the token still has to come from the client it was
    /// issued to: another client presenting it is a replay, not a parallel
    /// session. And a disabled account gets nothing, as on the live path.
    #[tokio::test]
    async fn the_grace_window_keeps_the_client_and_account_checks() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let stranger = store
            .register_oauth_client(
                "Another Client",
                None,
                &["https://x.example/cb".to_string()],
            )
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
            .await
            .unwrap();

        store.set_disabled("ada", true).await.unwrap();
        assert!(matches!(
            store
                .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 1)
                .await
                .unwrap(),
            RefreshOutcome::Unknown
        ));
        store.set_disabled("ada", false).await.unwrap();
        assert!(matches!(
            store
                .refresh_oauth_grant_at(
                    &issued.refresh_token,
                    &client.client_id,
                    Some("https://other.example"),
                    now + 1
                )
                .await
                .unwrap(),
            RefreshOutcome::WrongTarget
        ));
        assert!(matches!(
            store
                .refresh_oauth_grant_at(&issued.refresh_token, &stranger.client_id, None, now + 1)
                .await
                .unwrap(),
            RefreshOutcome::Replayed { .. }
        ));
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    }

    /// Every answer inside the window adds a live token, so the set is kept
    /// small: past `OAUTH_LIVE_TOKENS_KEPT` the oldest live tokens are
    /// deleted outright (an unknown token later, never a replay), and only
    /// `OAUTH_RETIRED_TOKENS_KEPT` rotated ones are remembered.
    #[tokio::test]
    async fn a_grant_keeps_a_bounded_set_of_tokens() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let now = chrono::Utc::now().timestamp();
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
            .await
            .unwrap();
        let mut siblings = Vec::new();
        for i in 0..12 {
            match store
                .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + i)
                .await
                .unwrap()
            {
                RefreshOutcome::Reissued { grant, .. } => siblings.push(grant),
                other => panic!("inside the window, got {other:?}"),
            }
        }
        let row = store
            .query_first(
                "SELECT COUNT(*) FROM oauth_grant_token WHERE rotated_at IS NULL",
                vec![],
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(row.get_value(0), Ok(Value::Integer(n)) if n == OAUTH_LIVE_TOKENS_KEPT));
        assert!(
            matches!(
                store
                    .refresh_oauth_grant_at(
                        &siblings[0].refresh_token,
                        &client.client_id,
                        None,
                        now + 20
                    )
                    .await
                    .unwrap(),
                RefreshOutcome::Unknown
            ),
            "a live token pushed out of the set is unknown, not a replay"
        );
        assert_eq!(count_rows(&store, "oauth_grants").await, 1);

        let mut current = siblings.last().unwrap().clone();
        for i in 0..12 {
            match store
                .refresh_oauth_grant_at(
                    &current.refresh_token,
                    &client.client_id,
                    None,
                    now + 30 + i,
                )
                .await
                .unwrap()
            {
                RefreshOutcome::Rotated(next) => current = next,
                other => panic!("a live token rotates, got {other:?}"),
            }
        }
        assert!(
            count_rows(&store, "oauth_grant_token").await
                <= OAUTH_LIVE_TOKENS_KEPT + OAUTH_RETIRED_TOKENS_KEPT
        );
    }

    /// The record of an ended grant: written for a replay (above) and for an
    /// explicit revoke here, listed newest first, left out after seven days,
    /// and never carrying a token.
    #[tokio::test]
    async fn an_explicit_revoke_is_recorded_and_listed_for_a_while() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let one = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        let two = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        assert!(store.revoke_oauth_grant("ada", one.id).await.unwrap());
        assert!(
            store
                .revoke_oauth_grant_by_token(&two.refresh_token, &client.client_id)
                .await
                .unwrap()
        );
        assert!(
            !store.revoke_oauth_grant("ada", one.id).await.unwrap(),
            "an ended grant cannot be revoked again"
        );
        assert_eq!(count_rows(&store, "oauth_grant_token").await, 0);
        let ended = store.list_ended_oauth_grants("ada").await.unwrap();
        let reasons: Vec<_> = ended
            .iter()
            .map(|g| (g.id, g.ended.as_ref().unwrap().reason))
            .collect();
        assert!(reasons.contains(&(one.id, OauthGrantEndReason::Revoked)));
        assert!(reasons.contains(&(two.id, OauthGrantEndReason::ClientRevoked)));
        assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());
        let json = serde_json::to_string(&ended).unwrap();
        assert!(json.contains("\"reason\":\"revoked\""), "{json}");
        assert!(!json.contains(OAUTH_ACCESS_PREFIX) && !json.contains(OAUTH_REFRESH_PREFIX));

        let later = chrono::Utc::now().timestamp() + OAUTH_ENDED_KEPT_SECS + 1;
        assert!(
            store
                .list_ended_oauth_grants_at("ada", later)
                .await
                .unwrap()
                .is_empty(),
            "after seven days the record is no longer listed"
        );
        store
            .add_user("bob", "Bob", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        assert!(
            store
                .list_ended_oauth_grants("bob")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A file written before 0.32.1 holds each grant's pair in `oauth_grants`
    /// itself, with the predecessor in `previous_refresh_hash`. Opening it
    /// moves both into `oauth_grant_token`: the access token keeps working,
    /// the refresh token rotates, and the predecessor is still a replay
    /// (its rotation time is read off the access token's issue, an hour ago
    /// here, far outside the window).
    #[tokio::test]
    async fn an_old_store_with_a_previous_refresh_hash_keeps_working() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        let access = format!("{OAUTH_ACCESS_PREFIX}{}", random_hex());
        let refresh = format!("{OAUTH_REFRESH_PREFIX}{}", random_hex());
        let previous = format!("{OAUTH_REFRESH_PREFIX}{}", random_hex());
        let id = {
            let store = AuthStore::open(&path).await.unwrap();
            let client = oauth_cast(&store).await;
            let now = chrono::Utc::now().timestamp();
            // What the 0.32.0 issue and rotation wrote, column by column.
            store
                .conn
                .execute(
                    "INSERT INTO oauth_grants
                         (user, client_id, resource, access_hash, access_expires_at,
                          refresh_hash, previous_refresh_hash, refresh_expires_at, created_at)
                     VALUES ('ada', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    vec![
                        Value::Text(client.client_id.clone()),
                        Value::Text(OAUTH_RESOURCE.to_string()),
                        Value::Text(token_hash(&access)),
                        Value::Integer(now + 60),
                        Value::Text(token_hash(&refresh)),
                        Value::Text(token_hash(&previous)),
                        Value::Integer(now + OAUTH_REFRESH_TTL_SECS),
                        Value::Text(chrono::Utc::now().to_rfc3339()),
                    ],
                )
                .await
                .unwrap();
            store.conn.last_insert_rowid()
        };
        let store = AuthStore::open(&path).await.unwrap();
        // A second open moves nothing twice.
        drop(store);
        let store = AuthStore::open(&path).await.unwrap();
        assert_eq!(count_rows(&store, "oauth_grant_token").await, 2);
        assert!(
            grant_text(&store, id, "refresh_hash")
                .await
                .starts_with(OAUTH_PLACEHOLDER_PREFIX)
        );
        let client_id = store.list_oauth_grants("ada").await.unwrap()[0]
            .client_id
            .clone();
        assert!(
            store
                .oauth_access_user(&access, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some(),
            "the access token minted before the upgrade still resolves"
        );
        let outcome = store
            .refresh_oauth_grant(&refresh, &client_id, None)
            .await
            .unwrap();
        let RefreshOutcome::Rotated(next) = outcome else {
            panic!("the refresh token minted before the upgrade rotates, got {outcome:?}");
        };
        assert_eq!(next.id, id);
        match store
            .refresh_oauth_grant(&previous, &client_id, None)
            .await
            .unwrap()
        {
            RefreshOutcome::Replayed { grant, .. } => assert_eq!(grant, id),
            other => panic!("the old predecessor is still a replay, got {other:?}"),
        }
    }

    /// The host on the consent screen and the grant card is a fact a person is
    /// asked to recognize, so a uri that names no host must not get to write
    /// that line itself.
    #[test]
    fn a_redirect_uri_with_no_host_never_shows_its_own_text() {
        assert_eq!(redirect_host("https://client.example/cb"), "client.example");
        assert_eq!(
            redirect_host("http://127.0.0.1:33418/callback"),
            "127.0.0.1:33418"
        );
        for uri in [
            "javascript:alert(1)",
            "data:text/html,<b>your bank</b>",
            "not a url at all",
        ] {
            assert_eq!(
                redirect_host(uri),
                NO_REDIRECT_HOST,
                "a uri with no host says so rather than speaking for itself: {uri}"
            );
        }
    }

    /// The empty string is not an audience. Refusing it at both ends means the
    /// two halves of the check cannot agree on nothing and let a token
    /// through, even if an origin derivation upstream ever produced one.
    #[tokio::test]
    async fn an_empty_resource_is_refused_on_both_sides() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        for resource in ["", "/", "   "] {
            assert!(
                store
                    .issue_oauth_grant("ada", &client.client_id, resource)
                    .await
                    .is_err(),
                "a grant with no audience is refused: {resource:?}"
            );
        }
        assert_eq!(count_rows(&store, "oauth_grants").await, 0);

        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE oauth_grants SET resource = '' WHERE id = ?1",
                vec![Value::Integer(issued.id)],
            )
            .await
            .unwrap();
        for resource in ["", "/"] {
            assert!(
                store
                    .oauth_access_user(&issued.access_token, resource)
                    .await
                    .unwrap()
                    .is_none(),
                "an empty origin opens nothing, whatever a row holds: {resource:?}"
            );
        }
    }

    /// A grant whose registration vanished stays listable, so it stays
    /// revocable: its tokens still resolve, and a list that dropped the row
    /// would leave the account no way to stop them.
    #[tokio::test]
    async fn a_grant_outlives_a_vanished_registration_rather_than_hiding() {
        let (_dir, store) = store().await;
        let client = oauth_cast(&store).await;
        let issued = store
            .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
            .await
            .unwrap();
        store
            .conn
            .execute("DELETE FROM oauth_clients", ())
            .await
            .unwrap();

        let listed = store.list_oauth_grants("ada").await.unwrap();
        assert_eq!(listed.len(), 1, "the grant is still listed");
        assert_eq!(listed[0].id, issued.id);
        assert!(
            !listed[0].client_name.is_empty(),
            "with something a person can read where the registration was"
        );
        assert!(store.revoke_oauth_grant("ada", issued.id).await.unwrap());
        assert!(
            store
                .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The resource comparison is the audience check, so its tolerance is
    /// exactly one trailing slash: nothing is lowercased and no other
    /// canonicalization happens, because an origin that differs in any other
    /// way is a different origin.
    #[test]
    fn a_resource_tolerates_one_trailing_slash_and_nothing_else() {
        assert_eq!(
            normalize_resource("https://crystal.example/"),
            "https://crystal.example"
        );
        assert_eq!(
            normalize_resource("  https://crystal.example  "),
            "https://crystal.example"
        );
        assert_eq!(
            normalize_resource("https://crystal.example//"),
            "https://crystal.example/",
            "one slash, not every slash"
        );
        assert_eq!(
            normalize_resource("https://Crystal.Example"),
            "https://Crystal.Example",
            "case is the caller's business, not the store's"
        );
        assert_eq!(normalize_resource("/"), "");
    }

    /// Every account a share-link test needs: the author who holds a draft,
    /// the colleague the link is for, and a third party who finds the link.
    async fn grant_cast(store: &AuthStore) {
        for name in ["alice", "bob", "carol"] {
            store
                .add_user(name, name, None, Role::Editor, "pw12345678")
                .await
                .unwrap();
        }
    }

    /// A share-link is for one person, and the first one to open it is that
    /// person for good.
    ///
    /// The whole of the sharing rule: a grant is minted for a draft rather
    /// than for a name, so the author never has to know the login of whoever
    /// they are handing it to - and the moment somebody opens it, it belongs
    /// to that account and to nobody else. A second account presenting the
    /// same token is told the same thing an invented one is told, because a
    /// link that admitted a second reader would be a draft shared with
    /// whoever the first reader forwarded it to.
    #[tokio::test]
    async fn a_grant_binds_its_first_redeemer_and_refuses_a_second() {
        let (_dir, store) = store().await;
        grant_cast(&store).await;
        let minted = store
            .mint_overlay_grant("team", "plan.md", "alice", None)
            .await
            .unwrap();
        assert!(minted.token.starts_with(DRAFT_LINK_PREFIX));

        let bound = store
            .redeem_overlay_grant(&minted.token, "bob")
            .await
            .unwrap()
            .expect("the first redeemer binds it");
        assert_eq!(
            (
                bound.domain.as_str(),
                bound.path.as_str(),
                bound.owner.as_str()
            ),
            ("team", "plan.md", "alice")
        );
        assert_eq!(bound.grantee.as_deref(), Some("bob"));

        assert!(
            store
                .redeem_overlay_grant(&minted.token, "bob")
                .await
                .unwrap()
                .is_some(),
            "and the person it bound to keeps it"
        );
        assert!(
            store
                .redeem_overlay_grant(&minted.token, "carol")
                .await
                .unwrap()
                .is_none(),
            "while anybody else is told what an invented token is told"
        );
        assert_eq!(
            store
                .overlay_grant_for("bob", "team", "plan.md")
                .await
                .unwrap()
                .as_deref(),
            Some("alice"),
            "the bound account sees whose draft it is"
        );
        assert!(
            store
                .overlay_grant_for("carol", "team", "plan.md")
                .await
                .unwrap()
                .is_none(),
            "and nobody else sees anything"
        );
    }

    /// The two ways a grant ends, and they end it the same way: the token
    /// still exists and resolves to nothing.
    #[tokio::test]
    async fn a_revoked_or_expired_grant_redeems_to_none() {
        let (_dir, store) = store().await;
        grant_cast(&store).await;

        let revoked = store
            .mint_overlay_grant("team", "plan.md", "alice", None)
            .await
            .unwrap();
        let listed = store
            .overlay_grants_of("alice", "team", "plan.md")
            .await
            .unwrap();
        assert_eq!(listed.len(), 1, "the author sees what they minted");
        assert!(
            store
                .revoke_overlay_grant("bob", revoked.id, false)
                .await
                .unwrap()
                .is_none(),
            "and nobody else can take it back"
        );
        let swept = store
            .mint_overlay_grant("team", "plan.md", "alice", None)
            .await
            .unwrap();
        assert!(
            store
                .revoke_overlay_grant("bob", swept.id, true)
                .await
                .unwrap()
                .is_some(),
            "an admin closes a link on this instance whoever minted it"
        );
        assert!(
            store
                .redeem_overlay_grant(&swept.token, "bob")
                .await
                .unwrap()
                .is_none(),
            "and the row was stamped, not only answered"
        );
        let taken = store
            .revoke_overlay_grant("alice", revoked.id, false)
            .await
            .unwrap()
            .expect("its author takes it back");
        assert_eq!(
            (
                taken.domain.as_str(),
                taken.owner.as_str(),
                taken.path.as_str()
            ),
            ("team", "alice", "plan.md"),
            "and the row says which draft it was on, which is what ends the sessions inside it"
        );
        assert!(
            taken.revoked_at.is_some(),
            "answered as it now stands, taken back"
        );
        assert!(
            store
                .revoke_overlay_grant("alice", revoked.id, false)
                .await
                .unwrap()
                .is_none(),
            "and taking back what is already back is nothing at all"
        );
        assert!(
            store
                .redeem_overlay_grant(&revoked.token, "bob")
                .await
                .unwrap()
                .is_none(),
            "a revoked link admits nobody"
        );

        let expired = store
            .mint_overlay_grant(
                "team",
                "plan.md",
                "alice",
                Some("2020-01-01T00:00:00Z".to_string()),
            )
            .await
            .unwrap();
        assert!(
            store
                .redeem_overlay_grant(&expired.token, "bob")
                .await
                .unwrap()
                .is_none(),
            "and neither does one whose window has closed"
        );

        let live = store
            .mint_overlay_grant("team", "plan.md", "alice", None)
            .await
            .unwrap();
        store
            .redeem_overlay_grant(&live.token, "bob")
            .await
            .unwrap()
            .expect("a live one still binds");
        assert_eq!(
            store.end_domain_overlay_grants("team").await.unwrap(),
            2,
            "and the domain leaving review mode ends every link nobody had \
             revoked: the live one and the expired one, whose window had closed \
             but whose row was still open"
        );
        assert!(
            store
                .overlay_grant_for("bob", "team", "plan.md")
                .await
                .unwrap()
                .is_none(),
            "so the account it was bound to holds nothing any more"
        );
    }

    /// The link is the credential, so it may never reach a log line - the rule
    /// [`IssuedMcpToken`] already keeps, kept the same way and asserted on the
    /// random half for the same reason.
    #[tokio::test]
    async fn minted_grant_debug_redacts_the_token() {
        let (_dir, store) = store().await;
        grant_cast(&store).await;
        let minted = store
            .mint_overlay_grant("team", "plan.md", "alice", None)
            .await
            .unwrap();
        let secret = minted
            .token
            .strip_prefix(DRAFT_LINK_PREFIX)
            .expect("a link carries the prefix");
        let text = format!("{minted:?}");
        assert!(!text.contains(secret), "the secret is redacted: {text}");
        assert!(text.contains("redacted"), "and says so: {text}");
        assert!(
            text.contains(&minted.id.to_string()),
            "while the id still prints: {text}"
        );
    }

    #[tokio::test]
    async fn one_github_id_links_to_one_account_per_host() {
        let (_dir, auth) = store().await;
        for name in ["ann", "bea"] {
            auth.add_user(name, name, None, Role::Editor, "pw12345678")
                .await
                .unwrap();
        }
        let link = auth
            .link_github("ann", "GitHub.com", 42, "ann-gh")
            .await
            .unwrap();
        assert_eq!(link.host, "github.com");
        assert_eq!(link.github_id, 42);
        assert!(link.token_live);
        let err = auth
            .link_github("bea", "github.com", 42, "other")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::GithubIdTaken)
        );
        assert!(err.to_string().contains(
            "this GitHub account is already linked to another Crystalline account; an admin can unlink it there"
        ));
        // The same id on another host is another person.
        auth.link_github("bea", "ghe.example.com", 42, "bea-ghe")
            .await
            .unwrap();
        assert_eq!(
            auth.account_for_github("github.com", 42)
                .await
                .unwrap()
                .as_deref(),
            Some("ann")
        );
        assert_eq!(
            auth.account_for_github("ghe.example.com", 42)
                .await
                .unwrap()
                .as_deref(),
            Some("bea")
        );
        assert_eq!(
            auth.account_for_github("github.com", 7).await.unwrap(),
            None
        );
        assert_eq!(auth.github_links().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn relinking_replaces_the_row() {
        let (_dir, auth) = store().await;
        auth.add_user("ann", "Ann", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.link_github("ann", "github.com", 1, "old")
            .await
            .unwrap();
        auth.mark_github_token_gone("ann").await.unwrap();
        let link = auth
            .link_github("ann", "github.com", 2, "new")
            .await
            .unwrap();
        assert_eq!(link.github_id, 2);
        assert!(link.token_live, "a fresh link has a live token");
        let all = auth.github_links().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].login, "new");
        assert_eq!(
            auth.account_for_github("github.com", 1).await.unwrap(),
            None
        );
        // The same id again is a relink of one's own row, not a conflict.
        auth.link_github("ann", "github.com", 2, "renamed")
            .await
            .unwrap();
        assert_eq!(
            auth.github_link_of("ann").await.unwrap().unwrap().login,
            "renamed"
        );
        auth.set_github_login("ann", "again").await.unwrap();
        assert_eq!(
            auth.github_link_of("ann").await.unwrap().unwrap().login,
            "again"
        );
        assert!(auth.unlink_github("ann").await.unwrap());
        assert!(!auth.unlink_github("ann").await.unwrap());
    }

    #[tokio::test]
    async fn removing_a_user_removes_its_link() {
        let (_dir, auth) = store().await;
        auth.add_user("root", "Root", None, Role::Admin, "pw12345678")
            .await
            .unwrap();
        for name in ["ann", "bea"] {
            auth.add_user(name, name, None, Role::Editor, "pw12345678")
                .await
                .unwrap();
        }
        auth.link_github("ann", "github.com", 5, "ann")
            .await
            .unwrap();
        auth.link_github("bea", "github.com", 6, "bea")
            .await
            .unwrap();
        auth.remove_user("ann").await.unwrap();
        auth.remove_user_force("bea").await.unwrap();
        assert!(auth.github_links().await.unwrap().is_empty());
        auth.add_user("cy", "Cy", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.link_github("cy", "github.com", 5, "cy").await.unwrap();
    }

    #[tokio::test]
    async fn marking_the_token_gone_keeps_the_link() {
        let (_dir, auth) = store().await;
        auth.add_user("ann", "Ann", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.link_github("ann", "github.com", 9, "ann")
            .await
            .unwrap();
        assert!(auth.mark_github_token_gone("ann").await.unwrap());
        let link = auth.github_link_of("ann").await.unwrap().unwrap();
        assert!(!link.token_live);
        assert_eq!(link.github_id, 9);
        assert!(!auth.mark_github_token_gone("nobody").await.unwrap());
    }

    #[tokio::test]
    async fn an_old_database_gains_the_table_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        {
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            let start = SCHEMA
                .find("CREATE TABLE IF NOT EXISTS github_link")
                .unwrap();
            let end = start + SCHEMA[start..].find("github_link_host_id").unwrap();
            let end = end + SCHEMA[end..].find(";\n").unwrap() + 2;
            let old = format!("{}{}", &SCHEMA[..start], &SCHEMA[end..]);
            assert!(!old.contains("github_link"));
            conn.execute_batch(&old).await.unwrap();
        }
        let auth = AuthStore::open(&path).await.unwrap();
        auth.add_user("ann", "Ann", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.link_github("ann", "github.com", 3, "ann")
            .await
            .unwrap();
        assert_eq!(auth.github_links().await.unwrap().len(), 1);
    }

    // ---- Per-domain Reflection collaborators (Task 3.4) ----

    async fn reflection_grant_cast(auth: &AuthStore) {
        for name in ["ann", "bea"] {
            auth.add_user(name, name, None, Role::Viewer, "pw12345678")
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn a_reflection_grant_is_stored_listed_and_removed() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        auth.set_reflection_grant("eng", "Ann", ReflectionLevel::Reader, "boss")
            .await
            .unwrap();
        auth.set_reflection_grant("eng", "bea", ReflectionLevel::Contributor, "boss")
            .await
            .unwrap();
        // A second write changes the level of the one row.
        auth.set_reflection_grant("eng", "ann", ReflectionLevel::Contributor, "keeper")
            .await
            .unwrap();
        let grants = auth.reflection_grants("eng").await.unwrap();
        let seen: Vec<(&str, ReflectionLevel, &str)> = grants
            .iter()
            .map(|g| (g.account.as_str(), g.level, g.added_by.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("ann", ReflectionLevel::Contributor, "keeper"),
                ("bea", ReflectionLevel::Contributor, "boss"),
            ]
        );
        assert_eq!(
            auth.reflection_grant("eng", "ann").await.unwrap(),
            Some(ReflectionLevel::Contributor)
        );
        assert_eq!(auth.reflection_grant("ops", "ann").await.unwrap(), None);
        assert!(auth.remove_reflection_grant("eng", "ANN").await.unwrap());
        assert!(!auth.remove_reflection_grant("eng", "ann").await.unwrap());
        assert_eq!(auth.reflection_grant("eng", "ann").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_default_grant_is_refused() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        let err = auth
            .set_reflection_grant("eng", "ann", ReflectionLevel::Default, "boss")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::EmptyMembership)
        );
        assert!(format!("{err}").contains("use remove"), "{err}");
        assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_grant_names_a_live_account() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        auth.set_disabled("bea", true).await.unwrap();
        for name in ["nobody", "bea", crate::scope::OWNER_IDENTITY_NAME] {
            let err = auth
                .set_reflection_grant("eng", name, ReflectionLevel::Reader, "boss")
                .await
                .unwrap_err();
            assert_eq!(
                StoreRefusal::kind_of(&err),
                Some(RefusalKind::NoSuchAccount),
                "{name}: {err}"
            );
        }
        assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
    }

    /// A shared domain has no acl, member or share-link row, so the grants
    /// alone must carry the rename. Stale grants under the new name go.
    #[tokio::test]
    async fn renaming_a_domain_carries_its_grants() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "boss")
            .await
            .unwrap();
        auth.set_reflection_grant("ops", "bea", ReflectionLevel::Contributor, "boss")
            .await
            .unwrap();
        auth.rename_domain("eng", "ops").await.unwrap();
        assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
        let moved: Vec<String> = auth
            .reflection_grants("ops")
            .await
            .unwrap()
            .into_iter()
            .map(|g| g.account)
            .collect();
        assert_eq!(moved, vec!["ann".to_string()]);
        // Again after a crash: nothing under the old name, nothing changes.
        auth.rename_domain("eng", "ops").await.unwrap();
        assert_eq!(auth.reflection_grants("ops").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn forgetting_a_domain_drops_its_grants() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "boss")
            .await
            .unwrap();
        auth.set_reflection_grant("ops", "ann", ReflectionLevel::Reader, "boss")
            .await
            .unwrap();
        assert!(!auth.forget_domain("eng").await.unwrap());
        assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
        assert_eq!(auth.reflection_grants("ops").await.unwrap().len(), 1);
    }

    /// A grant left behind would be inherited by the next account to take
    /// the name, as a membership row would.
    #[tokio::test]
    async fn removing_an_account_drops_its_grants() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        for name in ["ann", "bea"] {
            auth.set_reflection_grant("eng", name, ReflectionLevel::Reader, "boss")
                .await
                .unwrap();
        }
        auth.remove_user("ann").await.unwrap();
        auth.remove_user_force("bea").await.unwrap();
        auth.add_user("ann", "ann", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
        assert_eq!(auth.reflection_grant("eng", "ann").await.unwrap(), None);
    }

    /// Making a domain shared forgets its members, never its Reflection
    /// collaborators: a grant does not depend on the visibility.
    #[tokio::test]
    async fn a_grant_outlives_a_visibility_change_and_a_transfer() {
        let (_dir, auth) = store().await;
        reflection_grant_cast(&auth).await;
        auth.set_domain_visibility("eng", true, "bea")
            .await
            .unwrap();
        auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "bea")
            .await
            .unwrap();
        auth.add_user("cy", "cy", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        auth.transfer_domain("eng", "cy").await.unwrap();
        auth.set_domain_visibility("eng", false, "cy")
            .await
            .unwrap();
        assert_eq!(
            auth.reflection_grant("eng", "ann").await.unwrap(),
            Some(ReflectionLevel::Reader)
        );
    }
}
