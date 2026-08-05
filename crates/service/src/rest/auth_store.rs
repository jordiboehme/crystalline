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
//!    connection. `PRAGMA busy_timeout` is set on every connection so a writer
//!    waits instead of failing, no transaction spans more than the two
//!    statements of [`AuthStore::remove_user`], and the two multi-statement
//!    operations ([`AuthStore::remove_user`] and [`AuthStore::create_session`])
//!    take `BEGIN IMMEDIATE` so they serialize against each other rather than
//!    interleaving. Covering test:
//!    `two_stores_on_one_file_interleave_writes`.
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
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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

/// Read a role back out of a database row. An unrecognized value can only come
/// from a hand-edited or corrupted file, so it resolves to the least
/// privileged role rather than failing the whole read: an unreadable row must
/// never fail open.
fn role_from_db(s: &str) -> Role {
    s.parse().unwrap_or(Role::Viewer)
}

/// Fold a supplied user name to the one form this store keys on: trimmed of
/// surrounding whitespace and lowercased. Empty is rejected.
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
fn normalize_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("a user name cannot be empty");
    }
    Ok(trimmed.to_lowercase())
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
";

/// The columns every user read selects, in the order [`user_from_row`] decodes.
const USER_COLUMNS: &str = "name, display, email, role, disabled";

/// [`USER_COLUMNS`] qualified for the session join, where `users` is aliased
/// `u`. Same columns in the same order, so [`user_from_row`] decodes both.
const USER_COLUMNS_JOINED: &str = "u.name, u.display, u.email, u.role, u.disabled";

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
        let db = Builder::new_local(&path.to_string_lossy())
            .build()
            .await
            .with_context(|| format!("opening auth database {}", path.display()))?;
        let conn = db.connect().context("connecting to the auth database")?;
        // The CLI and the daemon write the same file. Wait rather than fail;
        // every transaction here is a single short statement.
        conn.execute("PRAGMA busy_timeout = 5000", ())
            .await
            .context("setting the auth database busy timeout")?;
        conn.execute_batch(SCHEMA)
            .await
            .context("creating the auth database schema")?;
        Ok(AuthStore {
            _db: db,
            conn,
            guard: tokio::sync::Mutex::new(()),
        })
    }

    /// Add an account with a password. The name is folded by
    /// [`normalize_name`], so `Ada` and `ada` are the same account. Errors if
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
        let name = normalize_name(name)?;
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

    /// Check a password. `None` covers every way this can fail to produce a
    /// login: unknown name, wrong password, a disabled account, an account
    /// with no password at all (one provisioned by [`AuthStore::ensure_user`]),
    /// and a name that will not normalize. They are deliberately
    /// indistinguishable, so a caller cannot leak which one it was.
    pub async fn verify_password(&self, name: &str, password: &str) -> Result<Option<User>> {
        let Ok(name) = normalize_name(name) else {
            return Ok(None);
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
            return Ok(None);
        };
        let user = user_from_row(&row);
        if user.disabled {
            return Ok(None);
        }
        let Some(hash) = cell_text(&row, 5) else {
            return Ok(None);
        };
        if verify_hash(hash, password.to_string()).await? {
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

    /// Replace an account's password. Errors if the account does not exist, so
    /// a mistyped name on the CLI is reported rather than silently ignored.
    pub async fn set_password(&self, name: &str, password: &str) -> Result<()> {
        let hash = hash_password(password).await?;
        self.update_user(
            "UPDATE users SET pass_hash = ?2 WHERE name = ?1",
            name,
            Value::Text(hash),
        )
        .await
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

    /// Disable or re-enable an account. Disabling leaves existing sessions in
    /// place but [`AuthStore::session_user`] stops honoring them, so the effect
    /// is immediate without having to hunt the session rows down.
    ///
    /// Disabling the last enabled admin is refused (see [`NOT_LAST_ADMIN`]);
    /// re-enabling never is, so the `?2 = 0` arm short-circuits the guard.
    pub async fn set_disabled(&self, name: &str, disabled: bool) -> Result<()> {
        self.update_guarded(
            &format!(
                "UPDATE users SET disabled = ?2 WHERE name = ?1 AND (?2 = 0 OR {NOT_LAST_ADMIN})"
            ),
            name,
            Value::Integer(i64::from(disabled)),
            "disable",
        )
        .await
    }

    /// Delete an account and every session it holds. Errors if there is no
    /// such account.
    ///
    /// The two deletes are one `BEGIN IMMEDIATE` transaction, sessions first.
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
        let name = normalize_name(name)?;
        let key = vec![Value::Text(name.clone())];
        let _guard = self.guard.lock().await;
        self.begin_immediate()
            .await
            .with_context(|| format!("removing user '{name}'"))?;
        let result = async {
            self.conn
                .execute("DELETE FROM sessions WHERE user_name = ?1", key.clone())
                .await
                .with_context(|| format!("removing sessions for user '{name}'"))?;
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
    /// The name is folded by [`normalize_name`], which matters most here: a
    /// header value of `Ada` must resolve to the existing `ada` rather than
    /// mint a second account at the default role, which would silently undo a
    /// disable or a demotion. The display name keeps the casing as sent.
    pub async fn ensure_user(&self, name: &str, role: Role) -> Result<User> {
        let display = name.trim().to_string();
        let name = normalize_name(name)?;
        let _guard = self.guard.lock().await;
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
        self.query_first(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
            vec![Value::Text(name.clone())],
        )
        .await?
        .map(|row| user_from_row(&row))
        .ok_or_else(|| anyhow!("user '{name}' vanished right after being provisioned"))
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
        let name = normalize_name(name)?;
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
        let csrf = cell_text(&row, 5).unwrap_or_default();
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

    /// Run a single-column update against one account, failing when the
    /// account does not exist.
    async fn update_user(&self, sql: &str, name: &str, value: Value) -> Result<()> {
        let name = normalize_name(name)?;
        let _guard = self.guard.lock().await;
        let changed = self
            .conn
            .execute(sql, vec![Value::Text(name.clone()), value])
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        Ok(())
    }

    /// [`AuthStore::update_user`] for a statement carrying the
    /// [`NOT_LAST_ADMIN`] guard, where zero rows changed has a second possible
    /// meaning: the edit was refused because it would have left no enabled
    /// admin. `verb` names the refused operation in that message.
    ///
    /// The follow-up existence probe only picks between the two messages -
    /// nothing was written either way - so it does not need to share the
    /// statement's transaction.
    async fn update_guarded(&self, sql: &str, name: &str, value: Value, verb: &str) -> Result<()> {
        let name = normalize_name(name)?;
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
    async fn finish(&self, result: Result<()>) -> Result<()> {
        match result {
            Ok(()) => {
                self.conn
                    .execute("COMMIT", ())
                    .await
                    .context("committing an auth database transaction")?;
                Ok(())
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

/// Decode the [`USER_COLUMNS`] prefix of a row.
fn user_from_row(row: &Row) -> User {
    User {
        name: cell_text(row, 0).unwrap_or_default(),
        display: cell_text(row, 1).unwrap_or_default(),
        email: cell_text(row, 2),
        role: role_from_db(&cell_text(row, 3).unwrap_or_default()),
        disabled: matches!(row.get_value(4), Ok(Value::Integer(i)) if i != 0),
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

/// Verify a password against a stored PHC string, on the blocking pool for the
/// same reason as [`hash_password`]. A hash this cannot parse verifies as
/// false rather than erroring: a corrupt row must fail closed.
async fn verify_hash(hash: String, password: String) -> Result<bool> {
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

    /// The brief's central concurrency requirement: the `crystalline users`
    /// CLI edits this file while the daemon serves from it. That is two
    /// processes on one database, which is the case `crystalline_index`'s
    /// turso store explicitly does not exercise (see this module's docs), so
    /// it gets a test of its own. Two `AuthStore` instances stand in for the
    /// two processes; each has its own `Database` and `Connection`.
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

        // The CLI disables; the daemon stops honoring the live session.
        cli.set_disabled("ada", true).await.unwrap();
        assert!(daemon.session_user(&session.token).await.unwrap().is_none());

        // The CLI removes the account entirely, sessions and all, while the
        // daemon still holds its own handle open.
        cli.set_disabled("ada", false).await.unwrap();
        cli.remove_user("ada").await.unwrap();
        assert!(daemon.session_user(&second.token).await.unwrap().is_none());
        assert!(daemon.list_users().await.unwrap().is_empty());
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
        let first = store.ensure_user("ada", Role::Viewer).await.unwrap();
        let second = store.ensure_user("ada", Role::Viewer).await.unwrap();
        assert_eq!(first.name, second.name);
        assert_eq!(first.role, second.role);
        assert_eq!(store.list_users().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ensure_user_keeps_an_admin_assigned_role() {
        let (_dir, store) = store().await;
        store.ensure_user("ada", Role::Viewer).await.unwrap();
        store.set_role("ada", Role::Admin).await.unwrap();
        let again = store.ensure_user("ada", Role::Viewer).await.unwrap();
        assert_eq!(again.role, Role::Admin);
    }

    #[tokio::test]
    async fn a_provisioned_user_has_no_password_to_log_in_with() {
        let (_dir, store) = store().await;
        store.ensure_user("ada", Role::Editor).await.unwrap();
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

        let same = store.ensure_user("Ada", Role::Viewer).await.unwrap();
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
            assert!(store.ensure_user(blank, Role::Viewer).await.is_err());
            // A login attempt is a `None`, not an error, like every other bad
            // credential.
            assert!(store.verify_password(blank, "pw").await.unwrap().is_none());
        }
        assert!(store.list_users().await.unwrap().is_empty());
    }

    #[test]
    fn normalize_name_trims_folds_and_rejects_empty() {
        assert_eq!(normalize_name("  AdA  ").unwrap(), "ada");
        assert_eq!(normalize_name("Ada").unwrap(), "ada");
        assert!(normalize_name("").is_err());
        assert!(normalize_name("   ").is_err());
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
}
