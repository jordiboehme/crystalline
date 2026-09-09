//! Where a GitHub access token lives once a machine has one: the OS
//! keychain when it works, a permissions-locked file on disk when it does
//! not, or a read-only view over a value the environment already supplied.
//!
//! [`TokenStore::resolve_and_load`] fuses the backend choice with the one
//! read that decides it: a single keychain `get_password` both picks the
//! store and hands back whatever token was already there, so a caller never
//! reads twice (once to probe, once to load) and a machine that has granted
//! keychain access is prompted at most once. [`TokenStore::save_resolving`]
//! is its write-side twin, writing straight through the keychain with no
//! pre-save probe read and landing in the file store only when the keychain
//! write itself fails. Keeping the chosen backend in the returned
//! `TokenStore` value means a single sign-in session always reads back what
//! it just wrote, even on a machine whose keychain the read judged unusable.
//!
//! [`TokenStore::env`] builds the third backend directly rather than through
//! a keychain read: this crate never reads the process environment itself
//! (`CRYSTALLINE_GITHUB_TOKEN` is read exactly once, in
//! `crystalline_service::overlay::EnvOverlay::from_process_env`), so a caller
//! that already has the value in hand constructs the store explicitly.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::RemoteError;

/// The keyring service name every Crystalline credential is stored under.
const KEYRING_SERVICE: &str = "crystalline";

/// How long one OS keychain call may take before the backend is treated as
/// unusable. Generous enough that a machine merely showing the user an
/// "allow access" dialog is not cut off mid-decision, short enough that a
/// wedged keychain daemon cannot make a sign-in look frozen forever.
///
/// Every keychain touch in this module goes through
/// [`keyring_call_with_timeout`] under this bound, so no caller - the daemon
/// least of all - can block on the platform keychain indefinitely.
const KEYRING_TIMEOUT: Duration = Duration::from_secs(15);

/// The file name the file-backed store writes within its state directory for
/// the instance credential. Byte-identical to what every existing install
/// already carries: a personal token gets its own name beside it rather than
/// changing this one.
const TOKEN_FILE_NAME: &str = "github-token.json";

/// The longest personal identity name a credential is addressed by. Account
/// names come from the auth layer already trimmed and lowercased, so this is a
/// sanity ceiling rather than a policy: a keyring account name and a file name
/// both stay comfortably short.
///
/// Public because the settings layer validates `github.agent_identity` - a
/// name that has to address a credential here - against
/// [`valid_identity_name`], and its refusal message names this ceiling. One
/// predicate, one limit, one place to change them.
pub const MAX_IDENTITY_NAME_BYTES: usize = 128;

/// Which credential a store addresses: the instance-wide token, or one
/// person's personal token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenIdentity {
    /// The machine-wide credential `crystalline connect github` manages.
    Instance,
    /// A personal credential bound to a crystalline account name (already
    /// trimmed and lowercased by the account layer) - or the fixed local
    /// name `owner` for the CLI/stdio machine owner.
    Personal(String),
}

/// A saved GitHub access token, together with who it belongs to and where.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredToken {
    /// The token itself, as GitHub issued it.
    pub access_token: String,
    /// The GitHub host this token is valid against: `github.com`, or a
    /// GitHub Enterprise Server hostname.
    pub host: String,
    /// The login of the user this token authenticates as.
    pub user: String,
    /// When this token was saved.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl StoredToken {
    /// The signed-in login for display, or `None` when it is unknown. Only
    /// [`TokenStore::Env`] ever produces an empty `user` (an
    /// environment-supplied token with no login lookup behind it); every
    /// other store fills it in at connect time, so this is the one place
    /// that distinction needs making, rather than every caller checking
    /// `user.is_empty()` itself.
    pub fn user_display(&self) -> Option<&str> {
        if self.user.is_empty() {
            None
        } else {
            Some(self.user.as_str())
        }
    }
}

/// Redacts `access_token`: a `StoredToken` reaches `Debug` output in a log
/// line, a panic message or a test failure far more easily than it reaches
/// `Display` (which this type has none of), so the derive is deliberately not
/// used here. Every other field is plain, non-secret metadata and prints as
/// normal.
impl std::fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredToken")
            .field("access_token", &"<redacted>")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Where a GitHub access token is persisted between runs. `Clone` so the
/// engine can cache a resolved store for the process lifetime and hand out
/// copies without re-resolving; the manual redacting `Debug` below stays.
#[derive(Clone)]
pub enum TokenStore {
    /// The OS-native credential store (Keychain, Credential Manager, the
    /// Secret Service), addressed by account name within the shared
    /// `crystalline` service.
    Keyring {
        /// `"github"` for GitHub.com, `"github:<host>"` for a GitHub
        /// Enterprise Server host.
        account: String,
    },
    /// A single JSON file, used when no working keychain backend is
    /// available (headless Linux with no session bus, most CI runners).
    File {
        /// The token file's path.
        path: PathBuf,
    },
    /// A token supplied directly by the `CRYSTALLINE_GITHUB_TOKEN`
    /// environment variable: read-only, since the environment is the source
    /// of truth and there is nothing here for `save` or `delete` to change.
    /// Never produced by [`TokenStore::resolve_and_load`] or
    /// [`TokenStore::save_resolving`]; built directly with [`TokenStore::env`]
    /// by a caller that already holds the value (see the module docs).
    ///
    /// Instance-only, always: the environment supplies the machine's own
    /// credential and never a personal one. One process serves everybody who
    /// reaches it, so a single environment variable cannot mean "alice's
    /// token" for one request and "bob's" for the next -
    /// [`TokenIdentity::Personal`] is resolved through the keyring or the
    /// file store and never through here.
    Env {
        /// The token value, exactly as `CRYSTALLINE_GITHUB_TOKEN` carries it.
        token: String,
        /// The GitHub host this token authenticates against: `github.com`,
        /// or a GitHub Enterprise Server hostname.
        host: String,
    },
}

/// Redacts the `token` field of [`TokenStore::Env`] the same way
/// [`StoredToken`]'s manual `Debug` redacts `access_token`: the other two
/// variants carry no secret, so their fields print as a derive would.
impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenStore::Keyring { account } => {
                f.debug_struct("Keyring").field("account", account).finish()
            }
            TokenStore::File { path } => f.debug_struct("File").field("path", path).finish(),
            TokenStore::Env { host, .. } => f
                .debug_struct("Env")
                .field("token", &"<redacted>")
                .field("host", host)
                .finish(),
        }
    }
}

impl TokenStore {
    /// Picks a backend for `host` and loads whatever token it already holds,
    /// in exactly one keychain read. `host` is `None` for GitHub.com,
    /// `Some(host)` for a GitHub Enterprise Server host; `fallback_dir` is the
    /// origins state directory the file store lives under.
    ///
    /// The single `get_password` both chooses the store and returns its
    /// contents: `Ok(json)` means the keychain works and already holds a
    /// token, so `(Keyring, Some(token))`; `Err(NoEntry)` means the keychain
    /// works but is empty, so `(Keyring, None)` (an absent item never prompts
    /// on macOS, so a machine that has not connected yet is re-read freely);
    /// anything else - including failing to even build the entry, and the read
    /// outrunning [`KEYRING_TIMEOUT`] - means the backend itself is unusable
    /// (headless Linux with no session bus, most CI runners, a wedged keychain
    /// daemon), so the file fallback plus whatever that file holds. Callers
    /// that need both a token and the backend choice - every origin operation
    /// and the offline connection probe - get both here without the old
    /// probe-then-load double read that made every such call two keychain
    /// touches, one dialog each until the user grants "Always Allow".
    pub fn resolve_and_load(
        host: Option<&str>,
        fallback_dir: &Path,
    ) -> Result<(TokenStore, Option<StoredToken>), RemoteError> {
        TokenStore::resolve_and_load_for(&TokenIdentity::Instance, host, fallback_dir)
    }

    /// [`TokenStore::resolve_and_load`] for one identity: the same single
    /// keychain read, against the account `account_for_identity` names and
    /// with the file fallback [`TokenStore::file_fallback_for`] names, so the
    /// instance credential and every personal one are separate credentials
    /// rather than one credential several callers overwrite in turn.
    ///
    /// A personal name that could escape the fallback directory is refused
    /// here, before any backend is touched.
    pub fn resolve_and_load_for(
        identity: &TokenIdentity,
        host: Option<&str>,
        fallback_dir: &Path,
    ) -> Result<(TokenStore, Option<StoredToken>), RemoteError> {
        Self::resolve_and_load_bounded(identity, host, fallback_dir, |account| {
            keyring_read(account, KEYRING_TIMEOUT)
        })
    }

    /// [`TokenStore::resolve_and_load_for`] with the keychain read injected,
    /// so a test can drive the unusable-backend branch - a timeout included -
    /// without touching the real platform keychain. The only production
    /// caller passes [`keyring_read`] under [`KEYRING_TIMEOUT`].
    fn resolve_and_load_bounded(
        identity: &TokenIdentity,
        host: Option<&str>,
        fallback_dir: &Path,
        read: impl FnOnce(&str) -> KeyringRead,
    ) -> Result<(TokenStore, Option<StoredToken>), RemoteError> {
        let account = account_for_identity_checked(identity, host)?;
        match read(&account) {
            KeyringRead::Found(json) => {
                Ok((TokenStore::Keyring { account }, Some(from_json(&json)?)))
            }
            KeyringRead::Empty => Ok((TokenStore::Keyring { account }, None)),
            KeyringRead::Failed(_) => {
                let store = TokenStore::file_fallback_for(identity, fallback_dir)?;
                let token = store.load()?;
                Ok((store, token))
            }
        }
    }

    /// Saves `token` for `host` and returns the store that now holds it,
    /// writing through the keychain when it works and the file under
    /// `fallback_dir` when it does not - without the probe read
    /// [`TokenStore::resolve_and_load`] does, since a save has the value in
    /// hand and learns the same thing from the write itself.
    ///
    /// The keychain is tried first with a direct `set_password` under
    /// [`KEYRING_TIMEOUT`]; any failure (or failing to build the entry, or
    /// serialize the token, or the write outrunning that bound) lands the
    /// token in the file store instead. The trade-off is deliberate: a
    /// keychain that
    /// reads fine but fails this one write puts the token in the file, and the
    /// user simply retries connect - judged far rarer and more recoverable
    /// than re-probing on every save, which is the prompt storm this module
    /// exists to avoid.
    pub fn save_resolving(
        host: Option<&str>,
        fallback_dir: &Path,
        token: &StoredToken,
    ) -> Result<TokenStore, RemoteError> {
        TokenStore::save_resolving_for(&TokenIdentity::Instance, host, fallback_dir, token)
    }

    /// [`TokenStore::save_resolving`] for one identity: the same keychain-first
    /// write, against the per-identity account and file names, so saving
    /// alice's personal token never disturbs the instance credential or bob's.
    ///
    /// A personal name that could escape the fallback directory is refused
    /// here, before any backend is touched: the refusal names the account and
    /// nothing is written.
    pub fn save_resolving_for(
        identity: &TokenIdentity,
        host: Option<&str>,
        fallback_dir: &Path,
        token: &StoredToken,
    ) -> Result<TokenStore, RemoteError> {
        Self::save_resolving_bounded(identity, host, fallback_dir, token, |account, json| {
            keyring_write(account, json, KEYRING_TIMEOUT)
        })
    }

    /// [`TokenStore::save_resolving_for`] with the keychain write injected,
    /// so a test can drive the unusable-backend branch - a timeout included -
    /// and prove the token lands in the file store, without touching the real
    /// platform keychain. The only production caller passes [`keyring_write`]
    /// under [`KEYRING_TIMEOUT`].
    fn save_resolving_bounded(
        identity: &TokenIdentity,
        host: Option<&str>,
        fallback_dir: &Path,
        token: &StoredToken,
        write: impl FnOnce(&str, String) -> Result<(), RemoteError>,
    ) -> Result<TokenStore, RemoteError> {
        let account = account_for_identity_checked(identity, host)?;
        if let Ok(json) = to_json(token)
            && write(&account, json).is_ok()
        {
            return Ok(TokenStore::Keyring { account });
        }
        let store = TokenStore::file_fallback_for(identity, fallback_dir)?;
        store.save(token)?;
        Ok(store)
    }

    /// The file-backed store for `identity` under `fallback_dir`, the single
    /// place a token file's location is derived so
    /// [`TokenStore::resolve_and_load_for`] and
    /// [`TokenStore::save_resolving_for`] never disagree about where the
    /// fallback lives. The instance file keeps its long-standing
    /// `github-token.json`; a personal one is
    /// `github-token-personal-<name>.json` beside it.
    ///
    /// Public because a host process that redirects the token directory - the
    /// service engine's store override - maps identities through here rather
    /// than rebuilding the naming scheme. It returns a `Result` for the same
    /// reason: this is where a name becomes a path, so this is where a name
    /// that could escape `fallback_dir` is refused.
    pub fn file_fallback_for(
        identity: &TokenIdentity,
        fallback_dir: &Path,
    ) -> Result<TokenStore, RemoteError> {
        check_identity(identity)?;
        let name = match identity {
            TokenIdentity::Instance => TOKEN_FILE_NAME.to_string(),
            TokenIdentity::Personal(name) => format!("github-token-personal-{name}.json"),
        };
        Ok(TokenStore::File {
            path: fallback_dir.join(name),
        })
    }

    /// Builds the read-only [`TokenStore::Env`] backend for a token the
    /// caller already holds (from `CRYSTALLINE_GITHUB_TOKEN`, read once by
    /// `crystalline_service::overlay::EnvOverlay`). `host` defaults to
    /// `"github.com"` when `None`, matching every other host-defaulting site
    /// in this module (see `account_for_identity`).
    pub fn env(token: impl Into<String>, host: Option<&str>) -> TokenStore {
        TokenStore::Env {
            token: token.into(),
            host: host
                .map(str::to_string)
                .unwrap_or_else(|| "github.com".to_string()),
        }
    }

    /// Saves `token`, replacing whatever was saved before.
    pub fn save(&self, token: &StoredToken) -> Result<(), RemoteError> {
        match self {
            TokenStore::Keyring { account } => {
                let json = to_json(token)?;
                keyring_write(account, json, KEYRING_TIMEOUT)
            }
            TokenStore::File { path } => save_file(path, token),
            TokenStore::Env { .. } => Err(env_read_only_error()),
        }
    }

    /// Loads the saved token, or `None` if nothing has been saved yet. The
    /// `Env` variant never has "nothing saved": it always synthesizes a
    /// token from the value it was built with, an empty `user` (unknown
    /// offline - nothing in this crate ever calls GitHub just to look up who
    /// an environment-supplied token belongs to) and a fresh `created_at`
    /// (never displayed anywhere; only `user`, `kind` and the fact that a
    /// token exists at all ever surface).
    pub fn load(&self) -> Result<Option<StoredToken>, RemoteError> {
        match self {
            TokenStore::Keyring { account } => match keyring_read(account, KEYRING_TIMEOUT) {
                KeyringRead::Found(json) => from_json(&json).map(Some),
                KeyringRead::Empty => Ok(None),
                KeyringRead::Failed(e) => Err(credential_error("load", e)),
            },
            TokenStore::File { path } => load_file(path),
            TokenStore::Env { token, host } => Ok(Some(StoredToken {
                access_token: token.clone(),
                host: host.clone(),
                user: String::new(),
                created_at: chrono::Utc::now(),
            })),
        }
    }

    /// Deletes the saved token. Deleting when nothing is saved is not an
    /// error.
    pub fn delete(&self) -> Result<(), RemoteError> {
        match self {
            TokenStore::Keyring { account } => keyring_delete(account, KEYRING_TIMEOUT),
            TokenStore::File { path } => delete_file(path),
            TokenStore::Env { .. } => Err(env_read_only_error()),
        }
    }

    /// `"keyring"`, `"file"` or `"environment"`, for doctor and status to
    /// report which backend is in play.
    pub fn kind(&self) -> &'static str {
        match self {
            TokenStore::Keyring { .. } => "keyring",
            TokenStore::File { .. } => "file",
            TokenStore::Env { .. } => "environment",
        }
    }
}

/// The keyring account name for `identity` at `host`, the single site the
/// account naming scheme lives:
///
/// - the instance credential keeps exactly the names it has always had,
///   `"github"` for GitHub.com and `"github:<host>"` for a GitHub Enterprise
///   Server host, so an install that connected before personal tokens existed
///   still resolves its token;
/// - a personal credential is namespaced away from those,
///   `"github-personal:<name>"` and `"github-personal:<name>:<host>"`.
///
/// Callers that may be handed an unvalidated name go through
/// [`account_for_identity_checked`] instead; this function assumes the name is
/// already sound.
fn account_for_identity(identity: &TokenIdentity, host: Option<&str>) -> String {
    match (identity, host) {
        (TokenIdentity::Instance, None) => "github".to_string(),
        (TokenIdentity::Instance, Some(host)) => format!("github:{host}"),
        (TokenIdentity::Personal(name), None) => format!("github-personal:{name}"),
        (TokenIdentity::Personal(name), Some(host)) => format!("github-personal:{name}:{host}"),
    }
}

/// [`account_for_identity`] with the name checked first, for the two resolving
/// constructors: they take an identity from a caller and must refuse a bad one
/// before a keychain entry is opened or a path is joined.
fn account_for_identity_checked(
    identity: &TokenIdentity,
    host: Option<&str>,
) -> Result<String, RemoteError> {
    check_identity(identity)?;
    Ok(account_for_identity(identity, host))
}

/// Whether `name` is safe to address a credential by: non-empty, at most
/// [`MAX_IDENTITY_NAME_BYTES`], and drawn only from `[a-z0-9._-]`. Account
/// names reach here already lowercased and in that shape from the auth layer,
/// so this is the belt to that layer's braces - and it is an allowlist rather
/// than a list of banned characters on purpose, because the characters that
/// hurt here are not enumerable in advance: `/` and `\` choose where a token
/// file lands, `:` is both this module's own account separator (so
/// `Personal("alice:ghes.example")` with no host would collide with
/// `Personal("alice")` at `ghes.example`) and the NTFS alternate-data-stream
/// separator, and a NUL truncates a path at the syscall boundary. An
/// allowlist answers all three and whatever the next one turns out to be.
///
/// Public so the one caller outside this crate that decides an identity name
/// before a credential exists - the `github.agent_identity` setting, which
/// names the account an HTTP-MCP share runs as - asks this predicate rather
/// than mirroring its character class and its ceiling. A mirror would drift,
/// and the drift would show up as a setting that saves and then cannot be
/// resolved.
pub fn valid_identity_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_IDENTITY_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// Refuses an identity whose name could address something other than its own
/// credential. [`RemoteError::Refused`] rather than a credential error: the
/// caller's own request is at fault, and the message names the account it
/// asked for so the caller can see which one. The token itself is never part
/// of this - or of any other - message.
fn check_identity(identity: &TokenIdentity) -> Result<(), RemoteError> {
    match identity {
        TokenIdentity::Instance => Ok(()),
        TokenIdentity::Personal(name) if valid_identity_name(name) => Ok(()),
        // The name quoted here is by construction one the allowlist REFUSED,
        // so it may carry anything at all: a newline, a NUL, a terminal
        // escape sequence, a kilobyte of it. It is sanitized rather than
        // interpolated raw ([`quotable`]).
        TokenIdentity::Personal(name) => Err(RemoteError::Refused(format!(
            "\"{}\" is not a usable account name for a personal GitHub token; use the account name Crystalline knows this person by.",
            quotable(name)
        ))),
    }
}

/// Makes a rejected identity name safe to quote back at the caller.
///
/// Everything outside printable ASCII becomes `?` and anything past
/// [`MAX_IDENTITY_NAME_BYTES`] characters is cut with a trailing `...`. This
/// is the one place a name that failed [`valid_identity_name`] is put into a
/// message, and that message travels to a terminal, a log line and a JSON
/// error body - none of which should be rewritten by a control byte or an
/// escape sequence somebody chose. Echoing the name at all is worth this much
/// care because seeing which request was refused is what makes the refusal
/// actionable.
fn quotable(name: &str) -> String {
    let mut out: String = name
        .chars()
        .take(MAX_IDENTITY_NAME_BYTES)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if name.chars().nth(MAX_IDENTITY_NAME_BYTES).is_some() {
        out.push_str("...");
    }
    out
}

/// What one bounded keychain read came back with, as owned data: the
/// closure that runs on the worker thread maps `keyring`'s own error type
/// here rather than sending it across, so nothing in the bound depends on
/// that type staying `Send`.
enum KeyringRead {
    /// The entry exists and holds this serialized token.
    Found(String),
    /// The backend works and holds nothing for this account. Never a
    /// prompt on macOS, so a machine that has not connected yet is re-read
    /// freely.
    Empty,
    /// The backend itself is unusable (no session bus, no keychain daemon),
    /// carrying the reason for the log line.
    Failed(String),
}

/// Runs one OS keychain call under a deadline: the single site every keychain
/// touch in this module goes through. Production passes [`KEYRING_TIMEOUT`];
/// the bound is a parameter so a test can prove the timeout behaviour in
/// milliseconds instead of seconds.
///
/// The closure runs on a dedicated thread and the caller waits for it with a
/// deadline. A thread rather than `tokio::task::spawn_blocking` because every
/// caller here is synchronous - `TokenStore::load`, `save` and `delete` and
/// both resolving constructors are called from sync code (the CLI's connect
/// commands, the engine's sync credential resolver) as well as from inside
/// the daemon's runtime, and a sync function cannot await a
/// `tokio::time::timeout`. One shape that behaves the same in and out of a
/// runtime beats two, one of which would never be exercised.
///
/// **The bound is the whole of the defence on a read.** The daemon's async
/// connect paths move the whole SAVE off the runtime with `spawn_blocking`, so
/// a wedged keychain write costs a blocking-pool thread and nothing else. A
/// read is called synchronously from inside the runtime (the sync credential
/// resolver every share and pull goes through), so a wedged keychain read
/// occupies a runtime worker for up to [`KEYRING_TIMEOUT`] before this bound
/// frees it. That is the reason the bound exists and the reason it is measured
/// in seconds rather than minutes; moving the reads behind `spawn_blocking`
/// means making the resolver async, which is a bigger change than the fault it
/// would soften.
///
/// A call that outruns the deadline leaves its thread behind, still parked in
/// the platform keychain. That is deliberate: the point of the bound is that
/// the CALLER stops waiting, and a wedged keychain call cannot be cancelled
/// from outside. One leaked thread per wedged call, on a path that is
/// attempted at most a handful of times per process, is the price.
fn keyring_call_with_timeout<T, F>(
    timeout: Duration,
    operation: &str,
    f: F,
) -> Result<T, RemoteError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    keyring_call_bounded(timeout, operation, f)
        .map_err(|reason| credential_error(operation, reason))
}

/// [`keyring_call_with_timeout`] with the reason left bare, for the one caller
/// that frames it itself.
///
/// [`keyring_read`] carries its failures inside [`KeyringRead::Failed`], which
/// its own callers then turn into an error naming what THEY were doing
/// ("could not load the GitHub token: ..."). Handing it an already-framed
/// sentence produced "could not load the GitHub token: could not read the
/// GitHub token: the OS keychain did not answer within 15s" - one fault
/// described twice. So the framing happens once, at whichever layer is
/// speaking.
fn keyring_call_bounded<T, F>(timeout: Duration, operation: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("crystalline-keyring".to_string())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(timeout) {
        Ok(value) => Ok(value),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                operation,
                timeout_secs = timeout.as_secs_f64(),
                "the OS keychain did not answer in time; treating the backend as unusable"
            );
            Err(format!(
                "the OS keychain did not answer within {:.0}s",
                timeout.as_secs_f64()
            ))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("the OS keychain call ended without an answer".to_string())
        }
    }
}

/// One bounded keychain read for `account`, the shape both
/// [`TokenStore::resolve_and_load_for`] and [`TokenStore::load`] read
/// through. A timeout is reported as [`KeyringRead::Failed`], which is the
/// same unusable-backend answer a machine with no keychain daemon gives, so
/// the file fallback takes over on both.
fn keyring_read(account: &str, timeout: Duration) -> KeyringRead {
    let owned = account.to_string();
    let read = keyring_call_bounded(timeout, "read", move || {
        match keyring::Entry::new(KEYRING_SERVICE, &owned) {
            Ok(entry) => match entry.get_password() {
                Ok(json) => KeyringRead::Found(json),
                Err(keyring::Error::NoEntry) => KeyringRead::Empty,
                Err(e) => KeyringRead::Failed(e.to_string()),
            },
            Err(e) => KeyringRead::Failed(e.to_string()),
        }
    });
    // Bare, because whoever asked for the read is the one who says so: see
    // [`keyring_call_bounded`].
    read.unwrap_or_else(KeyringRead::Failed)
}

/// One bounded keychain write for `account`, the shape both
/// [`TokenStore::save_resolving_for`] and [`TokenStore::save`] write
/// through. `Err` carries the reason, a timeout included.
fn keyring_write(account: &str, json: String, timeout: Duration) -> Result<(), RemoteError> {
    let owned = account.to_string();
    keyring_call_with_timeout(timeout, "save", move || {
        match keyring::Entry::new(KEYRING_SERVICE, &owned) {
            Ok(entry) => entry.set_password(&json).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    })?
    .map_err(|e| credential_error("save", e))
}

/// One bounded keychain delete for `account`. Deleting an entry that is not
/// there is not an error, exactly as it was before the bound.
fn keyring_delete(account: &str, timeout: Duration) -> Result<(), RemoteError> {
    let owned = account.to_string();
    keyring_call_with_timeout(timeout, "delete", move || {
        match keyring::Entry::new(KEYRING_SERVICE, &owned) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e.to_string()),
        }
    })?
    .map_err(|e| credential_error("delete", e))
}

/// Builds a [`RemoteError::Credential`] naming the attempted `operation`.
fn credential_error(operation: &str, source: impl std::fmt::Display) -> RemoteError {
    RemoteError::Credential {
        detail: format!("could not {operation} the GitHub token: {source}"),
    }
}

/// The refusal [`TokenStore::save`] and [`TokenStore::delete`] return for the
/// `Env` variant: the environment is the source of truth for this token, so
/// there is nothing here to save or delete until the variable is unset.
fn env_read_only_error() -> RemoteError {
    RemoteError::Credential {
        detail: "the GitHub token comes from the CRYSTALLINE_GITHUB_TOKEN environment variable and is read-only; unset it to manage a saved token".to_string(),
    }
}

fn to_json(token: &StoredToken) -> Result<String, RemoteError> {
    serde_json::to_string(token).map_err(|e| RemoteError::Credential {
        detail: format!("could not serialize the GitHub token: {e}"),
    })
}

fn from_json(json: &str) -> Result<StoredToken, RemoteError> {
    serde_json::from_str(json).map_err(|e| RemoteError::Credential {
        detail: format!("the saved GitHub token is not valid: {e}"),
    })
}

/// Writes `token` to `path` as JSON: a sibling temp file, permissioned
/// owner-only, then renamed into place, so a reader never sees a partial
/// file and no other local account can read the token in between.
fn save_file(path: &Path, token: &StoredToken) -> Result<(), RemoteError> {
    let json = to_json(token)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, json.as_bytes())?;
    set_owner_only(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_file(path: &Path) -> Result<Option<StoredToken>, RemoteError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => from_json(&contents).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn delete_file(path: &Path) -> Result<(), RemoteError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Restricts `path` to owner read/write only, so the plaintext token file
/// is not readable by other local accounts. A no-op on non-unix platforms,
/// which have no equivalent bit this crate manages directly.
#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), RemoteError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), RemoteError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_token() -> StoredToken {
        StoredToken {
            access_token: "gho_examplesecret".to_string(),
            host: "github.com".to_string(),
            user: "octocat".to_string(),
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        }
    }

    // The keyring arms of `resolve_and_load` and `save_resolving` are
    // intentionally never exercised here: proving them would mean reading,
    // writing or prompting the real platform keychain from the test suite,
    // which is exactly what this crate's tests must never do. Their file
    // fallback is covered by driving the `File` variant directly below and by
    // `file_fallback_store_round_trips_under_the_fallback_dir`, which exercises
    // the exact store both functions build on a machine with no usable keyring
    // backend.
    //
    // The bound itself IS exercised, through the injected-closure seams
    // (`resolve_and_load_bounded`, `save_resolving_bounded`) and through
    // `keyring_call_with_timeout` directly: a closure that sleeps stands in
    // for a wedged keychain and nothing in these four tests opens a
    // `keyring::Entry`.

    /// A keychain call that answers inside the bound passes its value
    /// straight through: the bound is a ceiling, not a delay.
    #[test]
    fn a_fast_keyring_call_passes_its_value_through() {
        let answered =
            keyring_call_with_timeout(Duration::from_secs(5), "read", || "in time".to_string())
                .expect("a call that answers inside the bound is not a failure");
        assert_eq!(answered, "in time");
    }

    /// A keychain call that outruns the bound is reported as a credential
    /// failure naming the timeout, rather than blocking the caller forever.
    #[test]
    fn a_keyring_call_past_the_bound_fails_with_the_timeout() {
        let err = keyring_call_with_timeout(Duration::from_millis(50), "read", || {
            std::thread::sleep(Duration::from_secs(30));
        })
        .expect_err("a call past the bound must not be waited out");
        assert!(
            err.to_string().contains("did not answer within"),
            "the failure names the timeout: {err}"
        );
    }

    /// The reason a wedged READ reports is framed once, by whoever asked for
    /// it. It used to be framed twice - "could not load the GitHub token:
    /// could not read the GitHub token: ..." - because the bounded call framed
    /// it for the read and `TokenStore::load` framed it again for the load.
    #[test]
    fn a_wedged_read_is_reported_once_and_not_twice() {
        let read = keyring_call_bounded(Duration::from_millis(50), "read", || {
            std::thread::sleep(Duration::from_secs(30));
        })
        .map(|()| KeyringRead::Empty)
        .unwrap_or_else(KeyringRead::Failed);
        let KeyringRead::Failed(reason) = read else {
            panic!("a call past the bound is a failure");
        };
        assert!(
            reason.contains("did not answer within"),
            "the reason names the timeout: {reason}"
        );
        assert!(
            !reason.contains("could not"),
            "and is bare, so the layer that speaks frames it once: {reason}"
        );
        let framed = credential_error("load", reason).to_string();
        assert!(
            framed.contains("could not load the GitHub token"),
            "the load says what IT was doing: {framed}"
        );
        assert!(
            !framed.contains("could not read the GitHub token"),
            "and does not carry the read's framing as well: {framed}"
        );
    }

    /// A keychain WRITE that outruns the bound falls through to the file
    /// store, the same branch a machine with no keychain daemon takes - the
    /// token is saved, not lost, and the returned store says where.
    #[test]
    fn a_keyring_save_past_the_bound_lands_in_the_file_store() {
        let dir = tempfile::tempdir().unwrap();
        let token = sample_token();
        let store = TokenStore::save_resolving_bounded(
            &TokenIdentity::Instance,
            None,
            dir.path(),
            &token,
            |_account, _json| {
                keyring_call_with_timeout(Duration::from_millis(50), "save", || {
                    std::thread::sleep(Duration::from_secs(30));
                })
            },
        )
        .expect("the file store takes over");

        assert_eq!(store.kind(), "file");
        assert_eq!(
            store.load().unwrap().unwrap().access_token,
            token.access_token,
            "the token the wedged keychain never took is on disk"
        );
    }

    /// A keychain READ that outruns the bound falls through to the file
    /// store too, so a wedged keychain degrades a status read rather than
    /// hanging it.
    #[test]
    fn a_keyring_read_past_the_bound_falls_back_to_the_file_store() {
        let dir = tempfile::tempdir().unwrap();
        let token = sample_token();
        let file = TokenStore::file_fallback_for(&TokenIdentity::Instance, dir.path()).unwrap();
        file.save(&token).unwrap();

        let (store, loaded) = TokenStore::resolve_and_load_bounded(
            &TokenIdentity::Instance,
            None,
            dir.path(),
            |_| {
                let timed_out =
                    keyring_call_with_timeout(Duration::from_millis(50), "read", || {
                        std::thread::sleep(Duration::from_secs(30));
                    })
                    .expect_err("the stand-in keychain never answers");
                KeyringRead::Failed(timed_out.to_string())
            },
        )
        .expect("the file store takes over");

        assert_eq!(store.kind(), "file");
        assert_eq!(loaded.unwrap().access_token, token.access_token);
    }

    #[test]
    fn personal_and_instance_tokens_live_in_separate_files() {
        let dir = tempfile::tempdir().unwrap();
        let instance = TokenStore::file_fallback_for(&TokenIdentity::Instance, dir.path()).unwrap();
        let alice = TokenStore::file_fallback_for(
            &TokenIdentity::Personal("alice".to_string()),
            dir.path(),
        )
        .unwrap();

        // The instance file name is the one existing installs already carry;
        // a personal token gets its own file beside it.
        match (&instance, &alice) {
            (TokenStore::File { path: instance }, TokenStore::File { path: alice }) => {
                assert_eq!(instance, &dir.path().join("github-token.json"));
                assert_eq!(
                    alice,
                    &dir.path().join("github-token-personal-alice.json"),
                    "a personal token lands in its own file"
                );
            }
            other => panic!("expected two file stores, got {other:?}"),
        }

        // Saving one leaves the other untouched: the two are separate
        // credentials, not two views of one.
        alice.save(&sample_token()).unwrap();
        assert_eq!(alice.load().unwrap(), Some(sample_token()));
        assert_eq!(
            instance.load().unwrap(),
            None,
            "the instance token is not what alice just saved"
        );
    }

    #[test]
    fn account_names_are_identity_and_host_scoped() {
        assert_eq!(
            account_for_identity(&TokenIdentity::Instance, None),
            "github"
        );
        assert_eq!(
            account_for_identity(&TokenIdentity::Instance, Some("ghes.example")),
            "github:ghes.example"
        );
        assert_eq!(
            account_for_identity(&TokenIdentity::Personal("alice".to_string()), None),
            "github-personal:alice"
        );
        assert_eq!(
            account_for_identity(
                &TokenIdentity::Personal("alice".to_string()),
                Some("ghes.example")
            ),
            "github-personal:alice:ghes.example"
        );
    }

    #[test]
    fn a_path_escaping_identity_is_refused() {
        // `../x` escapes the fallback directory; `alice:ghes.example` would
        // collide with `Personal("alice")` at the host `ghes.example` as the
        // very same keyring account (and names an NTFS data stream on
        // Windows); `Alice` is outside the lowercased shape the auth layer
        // hands over. The allowlist refuses all three the same way.
        for name in ["../x", "alice:ghes.example", "Alice"] {
            let dir = tempfile::tempdir().unwrap();
            let err = TokenStore::save_resolving_for(
                &TokenIdentity::Personal(name.to_string()),
                None,
                dir.path(),
                &sample_token(),
            )
            .unwrap_err();

            let msg = err.to_string();
            assert!(matches!(err, RemoteError::Refused(_)), "{name}: {err:?}");
            assert!(
                msg.contains(name),
                "the refusal names the account: {name}: {msg}"
            );
            assert!(
                !msg.contains("gho_"),
                "a refusal never carries the token: {name}: {msg}"
            );

            let entries: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(
                entries.is_empty(),
                "nothing was written: {name}: {entries:?}"
            );
        }
    }

    #[test]
    fn a_refused_name_is_sanitized_before_it_is_quoted_back() {
        // The name in this refusal is one the allowlist just rejected, so it
        // carries whatever the caller sent: a terminal escape that would
        // repaint a log line, a newline that would forge a second line, a NUL
        // that truncates at a syscall boundary, and more bytes than the
        // ceiling allows. None of it survives into the message.
        let hostile = format!(
            "a\u{1b}[2Jb\nc\u{0}d\u{7}{}",
            "z".repeat(MAX_IDENTITY_NAME_BYTES)
        );
        let dir = tempfile::tempdir().unwrap();
        let err = TokenStore::save_resolving_for(
            &TokenIdentity::Personal(hostile.clone()),
            None,
            dir.path(),
            &sample_token(),
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(matches!(err, RemoteError::Refused(_)), "{err:?}");
        assert!(
            msg.chars().all(|c| c.is_ascii_graphic() || c == ' '),
            "no control byte and no escape reaches the message: {msg:?}"
        );
        assert!(
            !msg.contains(&hostile),
            "the raw name is never echoed: {msg:?}"
        );
        assert!(
            msg.contains("a?[2Jb?c?d?"),
            "what was rejected is still recognizable: {msg:?}"
        );
        assert!(
            msg.contains("..."),
            "an over-long name is cut rather than pasted whole: {msg:?}"
        );
        assert!(
            msg.len() < hostile.len() + 200,
            "the message stays bounded: {} chars",
            msg.len()
        );
    }

    #[test]
    fn file_fallback_store_round_trips_under_the_fallback_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::file_fallback_for(&TokenIdentity::Instance, dir.path()).unwrap();
        assert_eq!(store.kind(), "file");
        // The fallback lands at the documented file name under the given dir,
        // so a save here is found again by a later resolve on the same machine.
        match &store {
            TokenStore::File { path } => assert_eq!(path, &dir.path().join(TOKEN_FILE_NAME)),
            other => panic!("expected a file store, got {other:?}"),
        }

        assert_eq!(store.load().unwrap(), None, "nothing saved yet");
        let token = sample_token();
        store.save(&token).unwrap();
        assert_eq!(store.load().unwrap(), Some(token));
    }

    #[test]
    fn token_store_clone_preserves_backend_and_redaction() {
        // The engine caches a cloned store, so a clone must keep each variant's
        // identity and, for the secret-carrying variant, its redaction.
        let file = TokenStore::File {
            path: PathBuf::from("/nonexistent/github-token.json"),
        };
        assert_eq!(file.clone().kind(), "file");

        let keyring = TokenStore::Keyring {
            account: "github".to_string(),
        };
        assert_eq!(keyring.clone().kind(), "keyring");

        let env = TokenStore::env("gho_SECRETSECRET", Some("ghe.example.com"));
        let cloned = env.clone();
        assert_eq!(cloned.kind(), "environment");
        let debugged = format!("{cloned:?}");
        assert!(!debugged.contains("SECRET"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
    }

    #[test]
    fn file_store_round_trips_save_load_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File {
            path: dir.path().join("github-token.json"),
        };
        assert_eq!(store.kind(), "file");

        assert_eq!(store.load().unwrap(), None, "nothing saved yet");

        let token = sample_token();
        store.save(&token).unwrap();
        assert_eq!(store.load().unwrap(), Some(token.clone()));

        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None, "deleted");

        // Deleting again when nothing is saved is not an error.
        store.delete().unwrap();
    }

    #[test]
    fn file_store_save_overwrites_the_previous_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File {
            path: dir.path().join("github-token.json"),
        };

        let mut token = sample_token();
        store.save(&token).unwrap();
        token.access_token = "gho_replacedsecret".to_string();
        store.save(&token).unwrap();

        assert_eq!(store.load().unwrap(), Some(token));
    }

    #[test]
    fn file_store_leaves_no_temp_file_behind_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File {
            path: dir.path().join("github-token.json"),
        };

        store.save(&sample_token()).unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["github-token.json".to_string()],
            "no leftover temp file: {entries:?}"
        );
    }

    #[test]
    fn file_store_creates_the_parent_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("further");
        let store = TokenStore::File {
            path: nested.join("github-token.json"),
        };

        store.save(&sample_token()).unwrap();

        assert_eq!(store.load().unwrap(), Some(sample_token()));
    }

    #[test]
    fn file_store_rejects_corrupt_json_as_a_credential_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github-token.json");
        std::fs::write(&path, b"not json").unwrap();
        let store = TokenStore::File { path };

        let err = store.load().unwrap_err();
        assert!(matches!(err, RemoteError::Credential { .. }), "{err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn file_store_writes_the_token_file_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("github-token.json");
        let store = TokenStore::File { path: path.clone() };

        store.save(&sample_token()).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected owner-only permissions, got {mode:o}");
    }

    #[test]
    fn debug_redacts_the_access_token_but_keeps_the_other_fields() {
        let debugged = format!("{:?}", sample_token());
        assert!(!debugged.contains("gho_examplesecret"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
        assert!(debugged.contains("octocat"), "{debugged}");
    }

    #[test]
    fn kind_reports_keyring_for_the_keyring_variant() {
        let store = TokenStore::Keyring {
            account: "github".to_string(),
        };
        assert_eq!(store.kind(), "keyring");
    }

    // --- the Env variant ------------------------------------------------------

    #[test]
    fn env_store_round_trips_the_token_and_host_with_an_empty_user() {
        let store = TokenStore::env("gho_SECRETSECRET", Some("ghe.example.com"));
        assert_eq!(store.kind(), "environment");

        let token = store
            .load()
            .unwrap()
            .expect("the env store always has a token");
        assert_eq!(token.access_token, "gho_SECRETSECRET");
        assert_eq!(token.host, "ghe.example.com");
        assert_eq!(token.user, "", "the login is unknown offline");
        assert_eq!(token.user_display(), None);
    }

    #[test]
    fn env_store_defaults_the_host_to_github_com() {
        let store = TokenStore::env("gho_SECRETSECRET", None);
        let token = store.load().unwrap().unwrap();
        assert_eq!(token.host, "github.com");
    }

    #[test]
    fn env_store_save_returns_the_guidance_error() {
        let store = TokenStore::env("gho_SECRETSECRET", None);
        let err = store.save(&sample_token()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CRYSTALLINE_GITHUB_TOKEN"), "{msg}");
        assert!(msg.contains("read-only"), "{msg}");
        assert!(matches!(err, RemoteError::Credential { .. }));
    }

    #[test]
    fn env_store_delete_returns_the_guidance_error() {
        let store = TokenStore::env("gho_SECRETSECRET", None);
        let err = store.delete().unwrap_err();
        assert!(
            err.to_string().contains("CRYSTALLINE_GITHUB_TOKEN"),
            "{err}"
        );
        assert!(matches!(err, RemoteError::Credential { .. }));
    }

    #[test]
    fn env_store_debug_never_shows_the_token_or_a_prefix_of_it() {
        let store = TokenStore::env("gho_SECRETSECRET", Some("ghe.example.com"));
        let debugged = format!("{store:?}");
        assert!(!debugged.contains("SECRET"), "{debugged}");
        assert!(debugged.contains("<redacted>"), "{debugged}");
        assert!(debugged.contains("ghe.example.com"), "{debugged}");

        // The synthesized `StoredToken` redacts the same way, through the
        // existing manual `Debug` impl.
        let token = store.load().unwrap().unwrap();
        let token_debugged = format!("{token:?}");
        assert!(!token_debugged.contains("SECRET"), "{token_debugged}");
        assert!(token_debugged.contains("<redacted>"), "{token_debugged}");
    }
}
