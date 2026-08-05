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
pub struct AuthStore {
    // Retained so the connection stays valid for as long as the store does.
    _db: Database,
    conn: Connection,
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
        Ok(AuthStore { _db: db, conn })
    }

    /// Add an account with a password. Errors if the name is already taken;
    /// the primary key is the guard, so two racing writers cannot both win.
    pub async fn add_user(
        &self,
        name: &str,
        display: &str,
        email: Option<&str>,
        role: Role,
        password: &str,
    ) -> Result<()> {
        let hash = hash_password(password).await?;
        self.conn
            .execute(
                "INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
                vec![
                    Value::Text(name.to_string()),
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
    /// login - unknown name, wrong password, a disabled account, or an account
    /// with no password at all (one provisioned by [`AuthStore::ensure_user`])
    /// - so a caller cannot accidentally distinguish them in its response.
    pub async fn verify_password(&self, name: &str, password: &str) -> Result<Option<User>> {
        let Some(row) = self
            .query_first(
                &format!("SELECT {USER_COLUMNS}, pass_hash FROM users WHERE name = ?1"),
                vec![Value::Text(name.to_string())],
            )
            .await?
        else {
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

    /// Change an account's role.
    pub async fn set_role(&self, name: &str, role: Role) -> Result<()> {
        self.update_user(
            "UPDATE users SET role = ?2 WHERE name = ?1",
            name,
            Value::Text(role.as_str().to_string()),
        )
        .await
    }

    /// Disable or re-enable an account. Disabling leaves existing sessions in
    /// place but [`AuthStore::session_user`] stops honoring them, so the effect
    /// is immediate without having to hunt the session rows down.
    pub async fn set_disabled(&self, name: &str, disabled: bool) -> Result<()> {
        self.update_user(
            "UPDATE users SET disabled = ?2 WHERE name = ?1",
            name,
            Value::Integer(i64::from(disabled)),
        )
        .await
    }

    /// Delete an account and every session it holds. Errors if there is no
    /// such account.
    pub async fn remove_user(&self, name: &str) -> Result<()> {
        let key = vec![Value::Text(name.to_string())];
        let changed = self
            .conn
            .execute("DELETE FROM users WHERE name = ?1", key.clone())
            .await
            .with_context(|| format!("removing user '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        self.conn
            .execute("DELETE FROM sessions WHERE user_name = ?1", key)
            .await
            .with_context(|| format!("removing sessions for user '{name}'"))?;
        Ok(())
    }

    /// Every account, by name. Names sort byte-wise, which is the ordering
    /// contract the rest of the workspace's text columns use.
    pub async fn list_users(&self) -> Result<Vec<User>> {
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
    pub async fn ensure_user(&self, name: &str, role: Role) -> Result<User> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO users
                     (name, display, email, role, pass_hash, disabled, created_at)
                 VALUES (?1, ?1, NULL, ?2, NULL, 0, ?3)",
                vec![
                    Value::Text(name.to_string()),
                    Value::Text(role.as_str().to_string()),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .with_context(|| format!("provisioning user '{name}'"))?;
        self.query_first(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
            vec![Value::Text(name.to_string())],
        )
        .await?
        .map(|row| user_from_row(&row))
        .ok_or_else(|| anyhow!("user '{name}' vanished right after being provisioned"))
    }

    /// Issue a session for an existing account, valid for `ttl_secs` from now.
    /// The returned token is the only unhashed copy; only its sha256 is
    /// written. A non-positive `ttl_secs` produces an already-expired session,
    /// which is how the expiry path is exercised without waiting.
    pub async fn create_session(&self, name: &str, ttl_secs: i64) -> Result<Session> {
        let exists = self
            .query_first(
                "SELECT 1 FROM users WHERE name = ?1",
                vec![Value::Text(name.to_string())],
            )
            .await?;
        if exists.is_none() {
            bail!("no such user: '{name}'");
        }
        let token = random_hex();
        let csrf = random_hex();
        let expires_at = chrono::Utc::now().timestamp().saturating_add(ttl_secs);
        self.conn
            .execute(
                "INSERT INTO sessions (token_hash, user_name, csrf, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                vec![
                    Value::Text(token_hash(&token)),
                    Value::Text(name.to_string()),
                    Value::Text(csrf.clone()),
                    Value::Integer(expires_at),
                ],
            )
            .await
            .with_context(|| format!("creating a session for user '{name}'"))?;
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
    pub async fn session_user(&self, token: &str) -> Result<Option<(User, String)>> {
        let now = chrono::Utc::now().timestamp();
        self.conn
            .execute(
                "DELETE FROM sessions WHERE expires_at <= ?1",
                vec![Value::Integer(now)],
            )
            .await
            .context("pruning expired sessions")?;
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
        let changed = self
            .conn
            .execute(sql, vec![Value::Text(name.to_string()), value])
            .await
            .with_context(|| format!("updating user '{name}'"))?;
        if changed == 0 {
            bail!("no such user: '{name}'");
        }
        Ok(())
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
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
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
        let raw = std::fs::read(&path).unwrap();
        let haystack = String::from_utf8_lossy(&raw);
        assert!(
            !haystack.contains(&s.token),
            "the raw token is never stored"
        );
        assert!(
            !haystack.contains("hunter2"),
            "the password is never stored"
        );
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
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
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
