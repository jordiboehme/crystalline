//! Users and server-side sessions for the REST API, in their own small
//! database beside the index.
//!
//! Deliberately separate from the engine's store: credentials are not
//! knowledge, they must survive a `reindex --full` that discards the index,
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
//!    whole file and the CLI cannot open it at all (see [`open_database`],
//!    which is where the interesting part of this lives). On top of that,
//!    `PRAGMA busy_timeout` is set on every connection so a writer waits
//!    instead of failing, no transaction spans more than the two statements of
//!    [`AuthStore::remove_user`], and the two multi-statement operations
//!    ([`AuthStore::remove_user`] and [`AuthStore::create_session`]) take
//!    `BEGIN IMMEDIATE` so they serialize against each other rather than
//!    interleaving. Covering test:
//!    `users_add_works_while_another_process_holds_the_auth_db` in the CLI's
//!    `tests/users.rs`, which is the only one that spawns a second process, and
//!    which is `#[cfg(unix)]` because turso 0.7.2 has no shared WAL
//!    coordination on the default Windows IO backend (that file says the whole
//!    of it). `two_stores_on_one_file_interleave_writes` below covers the
//!    ordering within one process and nothing about the locking.
//!
//!    Where layer 1 cannot be had, the fallback open is what runs, and a
//!    second process is then refused at open time: [`legacy_open_error`] turns
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
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use sha2::{Digest, Sha256};
use turso::{Builder, Connection, Database, Row, Value};

/// What a user may do. Ordered least to most privileged; the REST layer maps
/// each endpoint to the minimum role it accepts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
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
            "viewer" => Ok(Role::Viewer),
            "editor" => Ok(Role::Editor),
            "admin" => Ok(Role::Admin),
            other => Err(anyhow!(
                "unknown role '{other}': expected viewer, editor or admin"
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
pub fn normalize_account_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("a user name cannot be empty");
    }
    if trimmed.chars().any(char::is_whitespace) {
        bail!(
            "a login name cannot contain whitespace: pick a space-free name \
             and put the readable form in the display name"
        );
    }
    Ok(trimmed.to_lowercase())
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
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct User {
    /// The login name and primary key. Also the identity the trusted-header
    /// mode provisions against.
    #[schema(example = "ada")]
    pub name: String,
    /// Human-readable name for the UI.
    #[schema(example = "Ada Lovelace")]
    pub display: String,
    /// Optional contact address; never used for login.
    #[schema(example = "ada@example.com")]
    pub email: Option<String>,
    /// What this account may do.
    pub role: Role,
    /// A disabled account keeps its rows but can neither log in nor use an
    /// already-issued session.
    pub disabled: bool,
    /// When this account last resolved a session or arrived through the
    /// trusted header, RFC 3339. Null for an account never seen.
    #[schema(example = "2026-08-08T09:14:22Z")]
    pub last_seen: Option<String>,
}

/// One identity an external provider asserts, tied to one account.
///
/// `(issuer, subject)` is the durable key: a username, an address and a
/// display name are all mutable presentation data, and none of them may move
/// an account. An account may hold several links (one per issuer), and a link
/// points at exactly one account.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
pub struct IdentityLink {
    /// The provider that asserts this identity, as its ID tokens spell it.
    #[schema(example = "https://login.microsoftonline.com/<tenant>/v2.0")]
    pub issuer: String,
    /// The provider's stable identifier for the person.
    #[schema(example = "0f8fad5b-d9cb-469f-a165-70867728950e")]
    pub subject: String,
    /// When the link was made, RFC 3339.
    #[schema(example = "2026-09-07T09:14:22Z")]
    pub linked_at: String,
    /// Who made it: the account that linked it, an admin's name, or `jit` for
    /// a link a first sign-in created along with its account.
    #[schema(example = "jit")]
    pub linked_by: String,
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

/// One row of an account's MCP token list, for a management UI or CLI. Never
/// carries the token itself - only the hash is stored, so there is nothing to
/// show back after issuance.
#[derive(Clone, Debug, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemberLevel {
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
            "viewer" => Ok(MemberLevel::Viewer),
            "editor" => Ok(MemberLevel::Editor),
            "manager" => Ok(MemberLevel::Manager),
            other => Err(anyhow!(
                "unknown membership level '{other}': expected viewer, editor or manager"
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
    /// The principal names no account, or names one that is disabled. ONE
    /// variant for both on purpose: a surface that cannot read the user list
    /// must not be handed a probe for which of the two it is.
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
/// There is no `visibility` field: see [`VISIBILITY_PRIVATE`] for why the
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

/// One membership row: who was invited to a private domain, at what level, by
/// whom and when.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct DomainMember {
    /// The member's login name, folded by [`normalize_account_name`].
    pub principal: String,
    /// What this member may do here.
    pub level: MemberLevel,
    /// Who added or last changed this row. An audit field, stored as given:
    /// it is usually a login name but may name a non-account actor, the same
    /// latitude the identity-link plan gives `linked_by`.
    pub added_by: String,
    /// RFC 3339, when this row was last written.
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
/// same statement on an existing file.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS users (
    name TEXT PRIMARY KEY,
    display TEXT NOT NULL,
    email TEXT,
    role TEXT NOT NULL,
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
";

/// Prefix every MCP token is minted with, so a token is recognizable at a
/// glance and distinct from a session cookie or a CSRF value. The remainder is
/// 64 lowercase hex characters, 32 bytes from the OS CSPRNG.
pub const MCP_TOKEN_PREFIX: &str = "cmt_";

/// The columns every user read selects, in the order [`user_from_row`] decodes.
const USER_COLUMNS: &str = "name, display, email, role, disabled, last_seen_at";

/// [`USER_COLUMNS`] qualified for the session join, where `users` is aliased
/// `u`. Same columns in the same order, so [`user_from_row`] decodes both.
const USER_COLUMNS_JOINED: &str = "u.name, u.display, u.email, u.role, u.disabled, u.last_seen_at";

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
/// `experimental_multiprocess_wal(true)` (turso 0.7.2) replaces that lock with
/// a shared coordination file beside the database (`web-auth.db-tshm`): the
/// open adds `OpenFlags::NoLock` instead of locking the file and writers
/// coordinate through that mapping, so the busy timeout finally does the
/// waiting it was always meant to do. The rest is read out of the turso 0.7.2
/// and turso_core 0.7.2 sources (`Database::effective_open_flags_for_path`,
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
/// where turso 0.7.2's default IO backend reports no shared WAL coordination
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
        conn.execute_batch(SCHEMA)
            .await
            .context("creating the auth database schema")?;
        ensure_column(&conn, "users", "last_seen_at TEXT").await?;
        Ok(AuthStore {
            _db: db,
            conn,
            guard: tokio::sync::Mutex::new(()),
        })
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
        let name = normalize_account_name(name)?;
        // Hash before taking the lock: argon2 is CPU, not database.
        let hash = hash_password(password).await?;
        let _guard = self.guard.lock().await;
        self.conn
            .execute(
                "INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                vec![
                    Value::Text(name.clone()),
                    Value::Text(display.to_string()),
                    match email {
                        Some(e) => Value::Text(e.to_string()),
                        None => Value::Null,
                    },
                    Value::Text(role.as_str().to_string()),
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
    /// holds the write lock, exactly like [`NOT_LAST_ADMIN`].
    ///
    /// The name is folded by [`normalize_account_name`] like every other path, and a
    /// name that will not fold is refused before anything is written, so a typo
    /// does not consume the one slot there is. The password is hashed outside
    /// the lock, as in [`AuthStore::add_user`]: argon2 is CPU, not database.
    pub async fn add_first_admin(&self, name: &str, display: &str, password: &str) -> Result<bool> {
        let name = normalize_account_name(name)?;
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
        let Some(hash) = cell_text(&row, 6) else {
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
    /// (see [`NOT_LAST_ADMIN`]); promoting anyone never is, so the `?2 =
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
    /// Disabling the last enabled admin is refused (see [`NOT_LAST_ADMIN`]);
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
    /// MCP tokens and its private-domain memberships. Errors if there is no
    /// such account.
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
    /// Removing the last enabled admin is refused (see [`NOT_LAST_ADMIN`]).
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
            self.delete_identity_links_of(&name).await?;
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
    /// `users remove --force`. Sessions, tokens and memberships still go
    /// first, in the same `BEGIN IMMEDIATE` transaction, for the resurrection
    /// reasons the guarded remove documents.
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
            self.delete_identity_links_of(&name).await?;
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
        let display = name.trim().to_string();
        let name = normalize_account_name(name)?;
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
                bail!(
                    "refusing to provision '{name}': the account cap is reached \
                     (auth.max_users = {cap}). Remove unused accounts or raise the cap"
                );
            }
        }
        self.conn
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
        self.query_first(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
            vec![Value::Text(name.clone())],
        )
        .await?
        .map(|row| user_from_row(&row))
        .ok_or_else(|| anyhow!("user '{name}' vanished right after being provisioned"))
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
        let base = normalize_account_name(desired_name)?;
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
                bail!(
                    "refusing to provision an account for this sign-in: the account cap is \
                     reached (auth.max_users = {cap}). Remove unused accounts or raise the cap"
                );
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
        let csrf = cell_text(&row, 6).unwrap_or_default();
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
    /// [`token_hash`] helper a session token uses.
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
                bail!("no such user: '{user}'");
            }
            self.conn
                .execute(
                    "INSERT INTO mcp_tokens (user, token_hash, label, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    vec![
                        Value::Text(user.clone()),
                        Value::Text(hash),
                        Value::Text(label.to_string()),
                        Value::Text(created_at),
                    ],
                )
                .await
                .with_context(|| format!("issuing an mcp token for user '{user}'"))?;
            Ok(())
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
        if let Ok(Value::Integer(id)) = row.get_value(6) {
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
                bail!("no such mcp token '{id}' for user '{user}'");
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

    /// The visibility record of one domain: `Some` when it is private, `None`
    /// when it is shared, which is every domain nobody ever made private.
    ///
    /// The lookup is exact on the trimmed name ([`normalize_domain`]), so it
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
    /// `private = true` writes (or replaces) the acl row and leaves the
    /// existing membership alone, so changing the owner of an already-private
    /// domain does not empty it. The one row it does drop is the incoming
    /// owner's own membership, if it had one: an owner holds every level, so
    /// the row could now only say less than the truth, and it would come back
    /// to life the moment the domain is handed on again. This is the same step
    /// [`AuthStore::transfer_domain`] takes, for the same reason - the two
    /// paths that change an owner must agree. `private = false` deletes the acl
    /// row *and*
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
    pub async fn set_domain_visibility(
        &self,
        domain: &str,
        private: bool,
        owner: &str,
    ) -> Result<()> {
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
                return Ok(());
            }
            self.require_live_user(&owner).await?;
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
                "SELECT principal, level, added_by, added_at FROM domain_member
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
            });
        }
        Ok(out)
    }

    /// Invite `principal` to `domain` at `level`, or change the level it is
    /// already there at. `added_by` is recorded as given (trimmed): it is an
    /// audit field, usually the acting account's name, and it is not resolved
    /// against the users table because a non-account actor may legitimately
    /// grant membership. It may not be empty, though - a blank "invited by" is
    /// not an audit trail, and it is what a caller that forgot to pass one
    /// would write.
    ///
    /// Three things are refused, all inside the one transaction that also does
    /// the write so none of them can be raced past:
    ///
    /// * a domain that is not private, because a membership row there would
    ///   grant nothing and mean nothing (see `crate::scope`, where every
    ///   caller's right on a shared domain comes from its instance role);
    /// * a principal that is not an existing, enabled account, so a typo is
    ///   reported instead of leaving a row waiting for someone to claim that
    ///   name later;
    /// * the domain's own owner, which is the one row that could only ever
    ///   *reduce* what its holder may do.
    pub async fn upsert_domain_member(
        &self,
        domain: &str,
        principal: &str,
        level: MemberLevel,
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
                         (domain, principal_kind, principal, level, added_by, added_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    vec![
                        Value::Text(domain.clone()),
                        Value::Text(PRINCIPAL_USER.to_string()),
                        Value::Text(principal.clone()),
                        Value::Text(level.as_str().to_string()),
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

    /// Drop every identity linked to one account. Called inside the removal
    /// transaction: a link that outlived its account would hand the next
    /// account to claim the name somebody else's sign-on.
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
    }
}

fn cell_text(row: &Row, idx: usize) -> Option<String> {
    match row.get_value(idx) {
        Ok(Value::Text(s)) => Some(s),
        _ => None,
    }
}

/// 32 bytes from the OS CSPRNG, lowercase hex. Used for both the session token
/// and the CSRF token.
fn random_hex() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    crystalline_index::hex_lower(&bytes)
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
/// requests are sharing.
async fn hash_password(password: &str) -> Result<String> {
    let password = password.to_string();
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow!("hashing a password failed: {e}"))
    })
    .await
    .context("the password hashing task failed")?
}

#[cfg(test)]
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
    pub(crate) static VERIFICATIONS: std::cell::Cell<u64>;
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
pub(crate) async fn dummy_verify(password: &str) -> Result<bool> {
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
    #[cfg(test)]
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
    ///   (turso_core 0.7.2 `io/mod.rs`, where the trait default is `false`).
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

    #[test]
    fn normalize_account_name_trims_folds_and_rejects_empty() {
        assert_eq!(normalize_account_name("  AdA  ").unwrap(), "ada");
        assert_eq!(normalize_account_name("Ada").unwrap(), "ada");
        assert!(normalize_account_name("").is_err());
        assert!(normalize_account_name("   ").is_err());
        assert!(normalize_account_name("ada lovelace").is_err());
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
            ("owner", Role::Editor),
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
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        let acl = store.domain_visibility("lab").await.unwrap().unwrap();
        assert_eq!(acl.domain, "lab");
        assert_eq!(acl.owner, "owner");
        assert_eq!(
            store.private_domains().await.unwrap(),
            vec![DomainAcl {
                domain: "lab".into(),
                owner: "owner".into()
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
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
            .await
            .unwrap();
        assert_eq!(store.domain_members("lab").await.unwrap().len(), 1);
        store
            .set_domain_visibility("lab", false, "owner")
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
            .set_domain_visibility("lab", false, "owner")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn changing_the_owner_of_a_private_domain_keeps_its_members() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
            .await
            .unwrap();
        store
            .set_domain_visibility("lab", true, "out")
            .await
            .unwrap();
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

    #[tokio::test]
    async fn promoting_a_member_to_owner_drops_the_row_that_now_says_less() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        for name in ["mem", "out"] {
            store
                .upsert_domain_member("lab", name, MemberLevel::Viewer, "owner")
                .await
                .unwrap();
        }
        // The same step `transfer_domain` takes, on the other path that changes
        // an owner: the two must agree, or a demotion would resurrect a level
        // the domain has since outgrown.
        store
            .set_domain_visibility("lab", true, "mem")
            .await
            .unwrap();
        assert_eq!(
            store
                .domain_members("lab")
                .await
                .unwrap()
                .iter()
                .map(|m| m.principal.as_str())
                .collect::<Vec<_>>(),
            vec!["out"],
            "the incoming owner's viewer row is gone, everyone else stays"
        );
        assert!(store.memberships_of("mem").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transfer_domain_swaps_the_owner_and_drops_its_member_row() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "out", MemberLevel::Editor, "owner")
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
            .set_domain_visibility("lab", true, "owner")
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
            "owner",
            "a refused transfer leaves the owner alone"
        );
    }

    #[tokio::test]
    async fn membership_is_upserted_and_removed_by_name() {
        let (_dir, store) = store().await;
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "MEM", MemberLevel::Viewer, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Manager, "owner")
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
        assert_eq!(members[0].added_by, "owner");
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
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not private"), "{err}");
        store
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        let err = store
            .upsert_domain_member("lab", "ghost", MemberLevel::Viewer, "owner")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such user"), "{err}");
        let err = store
            .upsert_domain_member("lab", "owner", MemberLevel::Viewer, "owner")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("already holds every level"), "{err}");
        store.set_disabled("mem", true).await.unwrap();
        let err = store
            .upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("disabled"), "{err}");
        let err = store
            .upsert_domain_member("lab", "out", MemberLevel::Viewer, "  ")
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
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Manager, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "out", MemberLevel::Editor, "owner")
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
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
            .await
            .unwrap();
        store.remove_user("owner").await.unwrap();
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
            .add_user("owner", "owner", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            ""
        );
        assert!(store.memberships_of("owner").await.unwrap().is_empty());
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
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
            .await
            .unwrap();
        store.remove_user_force("owner").await.unwrap();
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
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
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
            .set_domain_visibility("lab", true, "owner")
            .await
            .unwrap();
        let owner = store
            .upsert_domain_member("lab", "owner", MemberLevel::Editor, "owner")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&owner),
            Some(RefusalKind::OwnerIsNotAMember)
        );
        assert!(format!("{owner:#}").contains("owns domain"), "{owner:#}");

        let ghost = store
            .upsert_domain_member("lab", "ghost", MemberLevel::Editor, "owner")
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
            .upsert_domain_member("lab", "mem", MemberLevel::Editor, "owner")
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

        // And a failure that is not one of the three carries no kind at all,
        // so the surfaces keep answering 500 for what is genuinely theirs.
        let malformed = store
            .upsert_domain_member("lab", "  ", MemberLevel::Editor, "owner")
            .await
            .unwrap_err();
        assert_eq!(StoreRefusal::kind_of(&malformed), None);
    }

    #[tokio::test]
    async fn the_membership_tables_survive_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        {
            let store = AuthStore::open(&path).await.unwrap();
            members_cast(&store).await;
            store
                .set_domain_visibility("lab", true, "owner")
                .await
                .unwrap();
            store
                .upsert_domain_member("lab", "mem", MemberLevel::Manager, "owner")
                .await
                .unwrap();
        }
        // The schema is idempotent DDL, so opening the same file again applies
        // it a second time and must change nothing.
        let store = AuthStore::open(&path).await.unwrap();
        assert_eq!(
            store.domain_visibility("lab").await.unwrap().unwrap().owner,
            "owner"
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
        assert!("owner".parse::<MemberLevel>().is_err());
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
}
