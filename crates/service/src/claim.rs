//! The CLI against an owned Postgres database (spec section L).
//!
//! A command that reaches a running daemon over the control socket works as
//! before. A command that would open its own engine on Postgres tries the
//! owner lock once ([`claim_database_for_command`]):
//!
//! - Held elsewhere: it refuses before anything opens ([`DatabaseOwned`]),
//!   naming the owner and where to run the command instead. Opening a store
//!   migrates, so this covers read commands too.
//! - Free: the command owns the database until `main` returns
//!   ([`release_command_database`]). In the daemon's order: the lock, the
//!   takeover of the instance before (its sessions ended, the grace waited
//!   out unless it released cleanly), the owner row with `command` set, then
//!   every store opened with the fence and the command's own tag. A lost lock
//!   ends the command with [`crate::daemon::EXIT_FENCED`].
//!
//! The read-only reports (`status`, `doctor` without `--fix`, `prompt`) never
//! take the lock and on Postgres never open the store, free or owned: only an
//! owner migrates ([`set_command_line`]).
//!
//! The Turso path never comes here: every function answers "nothing to
//! claim" for it.

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use crystalline_core::config::{AccountsLocation, GlobalConfig};
use crystalline_index::Store;
use tokio::sync::Mutex as TokioMutex;

use crate::rest::AuthStore;

/// A command refused because another instance owns the Postgres database.
/// Typed so a caller can tell it from a store that failed to open: the
/// sentence goes out as it is, never inside the "could not reach the index"
/// wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseOwned {
    /// The sentence: who owns the database and what to do instead.
    pub message: String,
}

impl std::fmt::Display for DatabaseOwned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DatabaseOwned {}

/// What `main` says about the command this process runs.
#[derive(Clone, Debug)]
struct CommandLine {
    /// The command as the owner row records it, for example
    /// `crystalline reindex`: the subcommand path, no values.
    label: String,
    /// The arguments as they were typed, for the refusal's example.
    #[cfg_attr(
        not(feature = "postgres"),
        allow(
            dead_code,
            reason = "only the postgres backend refuses a command with an example"
        )
    )]
    words: Vec<String>,
    /// A read-only report, which never takes the lock.
    report: bool,
}

static COMMAND: OnceLock<CommandLine> = OnceLock::new();

/// Say which command this process runs, once, before it opens anything:
/// `label` is what the owner row records (`crystalline reindex`), `words`
/// are the arguments as typed (for the example in a refusal), and `report`
/// marks a read-only report (`status`, `doctor` without `--fix`, `prompt`),
/// which never takes the database and on Postgres never opens it.
pub fn set_command_line(label: &str, words: Vec<String>, report: bool) {
    let _ = COMMAND.set(CommandLine {
        label: label.to_string(),
        words,
        report,
    });
}

/// The command label of this process; `crystalline` when `main` said none.
fn command_label() -> String {
    COMMAND
        .get()
        .map_or_else(|| "crystalline".to_string(), |c| c.label.clone())
}

/// The command as a person types it: `crystalline` and the arguments.
/// The paths `--db` and `--config` name are this machine's, so they are left
/// out of an example meant for another machine, and the value of a flag that
/// carries a secret is shown as `<value>`: the example lands on stderr and in
/// `status --json`.
#[cfg_attr(
    not(feature = "postgres"),
    allow(
        dead_code,
        reason = "only the postgres backend refuses a command with an example"
    )
)]
fn command_example() -> String {
    match COMMAND.get() {
        Some(c) if !c.words.is_empty() => {
            let words = without_secret_values(&without_local_paths(&c.words));
            format!("crystalline {}", words.join(" "))
        }
        Some(c) => c.label.clone(),
        None => "crystalline <command>".to_string(),
    }
}

/// `words` without `--db`, `--config` and their values (as `--db x` or
/// `--db=x`).
#[cfg_attr(
    not(feature = "postgres"),
    allow(
        dead_code,
        reason = "only the postgres backend refuses a command with an example"
    )
)]
fn without_local_paths(words: &[String]) -> Vec<String> {
    const LOCAL: [&str; 2] = ["--db", "--config"];
    let mut kept = Vec::new();
    let mut skip_value = false;
    for word in words {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if LOCAL.contains(&word.as_str()) {
            skip_value = true;
            continue;
        }
        if LOCAL
            .iter()
            .any(|flag| word.starts_with(&format!("{flag}=")))
        {
            continue;
        }
        kept.push(word.clone());
    }
    kept
}

/// Whether `--name` carries a secret: `token`, `password` or `secret` alone,
/// or any of them or `key` as the last part of the name (`--mcp-token`,
/// `--client-secret`, `--private-key`). A bare `--key` is an engram field's
/// name, never a secret.
#[cfg_attr(
    not(feature = "postgres"),
    allow(
        dead_code,
        reason = "only the postgres backend refuses a command with an example"
    )
)]
fn is_secret_flag(name: &str) -> bool {
    const SECRET: [&str; 3] = ["token", "password", "secret"];
    name.strip_prefix("--").is_some_and(|name| {
        SECRET.contains(&name)
            || SECRET
                .iter()
                .chain(&["key"])
                .any(|s| name.ends_with(&format!("-{s}")))
    })
}

/// `words` with the value of every secret-carrying flag (as `--token x` or
/// `--token=x`) replaced by `<value>`. The word after such a flag is masked
/// whatever it looks like, since a secret can start with `-`.
#[cfg_attr(
    not(feature = "postgres"),
    allow(
        dead_code,
        reason = "only the postgres backend refuses a command with an example"
    )
)]
fn without_secret_values(words: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut mask_value = false;
    for word in words {
        if std::mem::take(&mut mask_value) {
            kept.push("<value>".to_string());
            continue;
        }
        match word.split_once('=') {
            Some((flag, _)) if is_secret_flag(flag) => kept.push(format!("{flag}=<value>")),
            Some(_) => kept.push(word.clone()),
            None => {
                mask_value = is_secret_flag(word);
                kept.push(word.clone());
            }
        }
    }
    kept
}

fn is_report() -> bool {
    COMMAND.get().is_some_and(|c| c.report)
}

/// The database this process claimed for its command, held until
/// [`release_command_database`]. A handle: clones share the one claim.
#[derive(Clone)]
pub struct CommandDbOwner {
    #[cfg(feature = "postgres")]
    inner: Arc<Held>,
}

#[cfg(feature = "postgres")]
struct Held {
    owner: crystalline_index::postgres::PgOwner,
    instance_id: String,
    database: String,
    /// Set once the sealed vault is installed over this claim's side
    /// channel ([`CommandDbOwner::open_vault`]).
    vault: tokio::sync::OnceCell<()>,
}

impl CommandDbOwner {
    /// The settings of every Postgres connection the command opens: tagged
    /// with its instance id and fenced on the held lock.
    #[cfg(feature = "postgres")]
    pub fn session_options(&self) -> crystalline_index::postgres::PgSessionOptions {
        crate::standby::owner_session_options(&self.inner.instance_id, self.inner.owner.fence())
    }

    /// The side channel of the claim, which reads and saves the
    /// configuration document on the owner thread's own connection.
    #[cfg(feature = "postgres")]
    pub fn side(&self) -> crystalline_index::postgres::SideChannel {
        self.inner.owner.side()
    }

    /// Install the sealed vault over this claim's side channel, once, when
    /// this process has the secret key: the command then seals and opens
    /// its secrets in the database as the daemon does (spec section D). The
    /// key is checked against the rows first, as at a start. Without the key
    /// nothing is installed, and since the claim marked this process as one
    /// whose secrets live in the vault, every credential call fails instead
    /// of reaching the keychain or a file. Call it after the store open
    /// migrated the database.
    #[cfg(feature = "postgres")]
    pub(crate) async fn open_vault(&self) -> anyhow::Result<()> {
        use crystalline_remote::sealed::SealKey;
        self.inner
            .vault
            .get_or_try_init(|| async {
                match SealKey::from_env().map_err(|e| anyhow::anyhow!("{e}"))? {
                    Some(key) => {
                        crate::secret_start::open_vault_over(
                            self.side(),
                            &self.inner.database,
                            key,
                        )
                        .await?;
                    }
                    None => tracing::debug!(
                        "no secret key: this command reads and writes no sealed secret"
                    ),
                }
                anyhow::Ok(())
            })
            .await?;
        Ok(())
    }

    /// Write the owner row again: on a schema the store open just created
    /// the table in, the announce after the lock found none.
    #[cfg(feature = "postgres")]
    pub async fn announce(&self) -> anyhow::Result<()> {
        self.inner.owner.announce().await.map_err(|e| {
            anyhow::anyhow!(
                "could not record this command as the owner of the database {}: {e}",
                self.inner.database
            )
        })
    }
}

/// The claim this process holds, if any. A tokio mutex, so two opens in one
/// command never race to claim twice.
static CLAIM: TokioMutex<Option<CommandDbOwner>> = TokioMutex::const_new(None);

/// Claim the Postgres database of `cfg` for `command`, or say who owns it.
///
/// `Ok(None)` on Turso (and in a build without the Postgres backend). A
/// second call in the same process answers the claim it already holds. When
/// another session holds the lock the error is a [`DatabaseOwned`] naming
/// the owner and how to run the command there, and nothing was opened.
pub async fn claim_database_for_command(
    cfg: &GlobalConfig,
    command: &str,
) -> anyhow::Result<Option<CommandDbOwner>> {
    match claim(cfg, command).await? {
        Claim::NotPostgres => Ok(None),
        Claim::Held(held) => Ok(Some(held)),
        #[cfg(feature = "postgres")]
        Claim::Owned { database, owner } => Err(anyhow::Error::new(DatabaseOwned {
            message: pg::command_refusal(&database, owner.as_ref(), &command_example()),
        })),
    }
}

/// Give the database back, when this process claimed it: `released_at` in
/// the owner row and the unlock, so the next instance takes over without a
/// grace. A failure is said in the log; the exit frees the lock anyway.
pub async fn release_command_database() {
    let held = CLAIM.lock().await.take();
    #[cfg(feature = "postgres")]
    if let Some(held) = held
        && let Err(e) = held.inner.owner.release().await
    {
        tracing::warn!("could not release the database ({e}); the exit frees it");
    }
    #[cfg(not(feature = "postgres"))]
    let _ = held;
}

/// What a lock attempt found.
#[cfg_attr(
    not(feature = "postgres"),
    allow(dead_code, reason = "only the postgres backend holds a claim")
)]
enum Claim {
    NotPostgres,
    Held(CommandDbOwner),
    #[cfg(feature = "postgres")]
    Owned {
        database: String,
        owner: Option<crystalline_index::postgres::OwnerRow>,
    },
}

#[cfg(not(feature = "postgres"))]
async fn claim(_cfg: &GlobalConfig, _command: &str) -> anyhow::Result<Claim> {
    Ok(Claim::NotPostgres)
}

#[cfg(feature = "postgres")]
async fn claim(cfg: &GlobalConfig, command: &str) -> anyhow::Result<Claim> {
    use crystalline_core::config::DatabaseBackend;
    use crystalline_index::postgres::{Acquired, OwnerIdentity, PgOwner, PgTarget};

    let database = cfg.database();
    if database.backend != DatabaseBackend::Postgres {
        return Ok(Claim::NotPostgres);
    }
    // On Postgres the secrets of this process live in the sealed vault
    // (spec section D), installed once the store is open; until then, and
    // for good without the key, a credential call fails rather than fall
    // back to the keychain or a file.
    crystalline_remote::vault::require();
    let mut slot = CLAIM.lock().await;
    if let Some(held) = slot.as_ref() {
        return Ok(Claim::Held(held.clone()));
    }
    let target = PgTarget::from_config(&database).map_err(|e| anyhow::anyhow!("{e}"))?;
    let described = target.describe();
    // An id of this process's own (see `pg::process_instance_id`): the
    // takeover ends every session of the database not tagged with it, so a
    // paused daemon of this machine is ended too.
    let machine = crystalline_core::config::read_or_create_instance_id().ok();
    let instance_id = pg::process_instance_id(machine.as_deref());
    let me = OwnerIdentity {
        instance_id: instance_id.clone(),
        label: cfg
            .instance_name()
            .or(machine)
            .unwrap_or_else(|| instance_id.clone()),
        command: Some(command.to_string()),
        version: crystalline_core::VERSION.to_string(),
    };
    let owner = PgOwner::start(target, me).map_err(|e| anyhow::anyhow!("{e}"))?;
    let previous = match owner.try_acquire().await {
        Ok(Acquired::Held { previous, .. }) => previous,
        Ok(Acquired::Busy(row)) => {
            return Ok(Claim::Owned {
                database: described,
                owner: row,
            });
        }
        Err(e) => anyhow::bail!("could not reach the database {described} to take it: {e}"),
    };
    // From the lock on, a lost lock ends the command, as it ends a daemon.
    crate::standby::watch_fence(&owner, described.clone());
    // Before any store opens: the instance before must not write beside
    // this command, so its sessions end and the grace is waited out unless
    // it released cleanly.
    let report = owner
        .takeover(previous.as_ref())
        .await
        .map_err(|e| anyhow::anyhow!("{}", crate::standby::plain(e)))?;
    let released = previous.as_ref().is_some_and(|p| p.released_at.is_some());
    tracing::debug!(
        "{}",
        crate::standby::takeover_line(&described, &report, released)
    );
    owner.announce().await.map_err(|e| {
        anyhow::anyhow!(
            "could not record this command as the owner of the database {described}: {e}"
        )
    })?;
    let held = CommandDbOwner {
        inner: Arc::new(Held {
            owner,
            instance_id,
            database: described,
            vault: tokio::sync::OnceCell::new(),
        }),
    };
    *slot = Some(held.clone());
    Ok(Claim::Held(held))
}

/// Attach the configuration document to `loaded` under `claim` (spec
/// section C): a command that owns the database reads the document once,
/// after its store open migrated the table, and every configuration write
/// it makes goes there. Nothing to do in file mode or when a document is
/// attached already.
///
/// A document that cannot be read is an error, and so is document mode
/// without a claim: a command that went on with `config.yaml` alone would
/// save the settings it does not know over the stored ones, or into the
/// read-only base.
pub(crate) async fn attach_owned_document(
    loaded: &mut crystalline_engine::overlay::LoadedConfig,
    claim: Option<&CommandDbOwner>,
) -> anyhow::Result<()> {
    if loaded.mode != crystalline_engine::overlay::ConfigMode::Document || loaded.document.is_some()
    {
        return Ok(());
    }
    #[cfg(feature = "postgres")]
    if let Some(claim) = claim {
        let side = claim.side();
        let body = side.load_config().await.map_err(|e| {
            anyhow::anyhow!(
                "could not read the configuration in the database {} ({e}); on Postgres the configuration lives in the database, so this command does not run without it",
                claim.inner.database
            )
        })?;
        return loaded.attach_document(std::sync::Arc::new(side), body.map(|d| d.body));
    }
    let _ = claim;
    anyhow::bail!(
        "the configuration is kept in the database, and this command does not own the database, so it cannot read it"
    )
}

/// The configuration of a command that opened the database under its claim:
/// [`crystalline_engine::overlay::load`], with the configuration document
/// attached in document mode. What a command that walks the registered
/// domains itself (a daemonless `sync` or `reindex`) reads once its store is
/// open, so the domains registered in the database are among them.
pub async fn owned_config(
    config_path: Option<&Path>,
) -> anyhow::Result<crystalline_engine::overlay::LoadedConfig> {
    let mut loaded = crystalline_engine::overlay::load(config_path)?;
    attach_owned_document(&mut loaded, held_claim().await.as_ref()).await?;
    Ok(loaded)
}

/// The claim this process holds for its command, if any.
pub(crate) async fn held_claim() -> Option<CommandDbOwner> {
    CLAIM.lock().await.clone()
}

/// Claim the database for `command` the way an embedded MCP stack does: as
/// [`claim_database_for_command`], but an owned database is refused with the
/// sentence of a second instance started by mistake (spec section O), which
/// an MCP client reads as the initialize error.
pub(crate) async fn claim_database_for_session(
    cfg: &GlobalConfig,
    command: &str,
) -> anyhow::Result<Option<CommandDbOwner>> {
    match claim(cfg, command).await? {
        Claim::NotPostgres => Ok(None),
        Claim::Held(held) => Ok(Some(held)),
        #[cfg(feature = "postgres")]
        Claim::Owned { database, owner } => Err(anyhow::Error::new(DatabaseOwned {
            message: pg::not_served_here(&database, owner.as_ref()),
        })),
    }
}

/// Open the store of `cfg` under `claim`: with the claim's tag and fence,
/// and the owner row written again once the store open made sure its table
/// exists. Without a claim, the plain open.
pub(crate) async fn open_store_as(
    claim: Option<&CommandDbOwner>,
    cfg: &GlobalConfig,
    db: Option<&Path>,
    resilient: bool,
) -> anyhow::Result<Arc<TokioMutex<dyn Store>>> {
    #[cfg(feature = "postgres")]
    if let Some(claim) = claim {
        let store = crystalline_index::open_store_with(
            &cfg.database(),
            db,
            resilient,
            &claim.session_options(),
        )
        .await?;
        claim.announce().await?;
        claim.open_vault().await?;
        return Ok(store);
    }
    #[cfg(not(feature = "postgres"))]
    let _ = claim;
    Ok(crystalline_index::open_store(&cfg.database(), db, resilient).await?)
}

/// Open the store of `cfg` for this process's command, by the CLI rule: on
/// Postgres the database is claimed first (refused with [`DatabaseOwned`]
/// when another instance owns it), and a read-only report never opens it
/// ([`DatabaseNotOpened`]). Turso opens as before.
pub async fn open_store_for_command(
    cfg: &GlobalConfig,
    db: Option<&Path>,
    resilient: bool,
) -> anyhow::Result<Arc<TokioMutex<dyn Store>>> {
    let claim = claim_for_this_command(cfg).await?;
    open_store_as(claim.as_ref(), cfg, db, resilient).await
}

/// Mark this process as one whose secrets live in the sealed vault when
/// `cfg` says Postgres (spec section D): from then on no credential call
/// reads or writes the keychain or the state folder, whatever path asks.
/// Without an installed vault such a call fails with
/// [`crystalline_remote::RemoteError::VaultUnavailable`], and a report shows
/// the secret as sealed instead. Turso: nothing changes.
pub fn keep_secrets_off_local_stores(cfg: &GlobalConfig) {
    if cfg.database().backend == crystalline_core::config::DatabaseBackend::Postgres {
        crystalline_remote::vault::require();
    }
}

/// [`keep_secrets_off_local_stores`] for the configuration this process's
/// command reads: the file its `--config` names, else `CRYSTALLINE_CONFIG`
/// or the default one. Read once at a CLI process's start, after
/// [`set_command_line`], and quietly: the command's own load says what is
/// wrong with the file. A configuration that does not load changes nothing
/// here.
pub fn keep_secrets_off_local_stores_for_command() {
    let config_path = command_config_path();
    let loaded =
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            crystalline_engine::overlay::load(config_path.as_deref())
        });
    if let Ok(loaded) = loaded {
        keep_secrets_off_local_stores(&loaded.effective);
    }
}

/// The value of the `--config` flag among the command's words (as
/// `--config x` or `--config=x`), the last one when it is given twice. A
/// word after `--` is a value, never a flag.
fn command_config_path() -> Option<std::path::PathBuf> {
    config_flag(&COMMAND.get()?.words)
}

fn config_flag(words: &[String]) -> Option<std::path::PathBuf> {
    let mut found = None;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if word == "--config" {
            found = words.next().map(std::path::PathBuf::from);
        } else if let Some(value) = word.strip_prefix("--config=") {
            found = Some(std::path::PathBuf::from(value));
        }
    }
    found
}

/// Where a CLI command that changes a secret (a GitHub token, a server
/// sign-in) makes the change (spec section D and the owner decision of
/// 2026-11-16).
pub enum SecretRoute {
    /// Turso: the keychain or the state folder, as before.
    Local,
    /// A daemon of this machine owns the Postgres database: the change goes
    /// to it over the control socket, and this process needs no key.
    Daemon,
    /// This command claimed the free Postgres database, and the sealed
    /// vault is installed with its key: it changes the secret itself, as
    /// the daemon would.
    Owned(CommandDbOwner),
    /// A daemon of this machine owns the Postgres database, and this
    /// command seals the secret itself with its own key, on a connection of
    /// its own beside the daemon ([`route_sealing_command`]). It tells the
    /// daemon before and after the write, so the daemon reads the secret
    /// again from the database: the secret never travels over the socket.
    ///
    /// Two writers, one row: a save of the daemon (a token it refreshes)
    /// and this command's land in the order they commit, and the last one
    /// wins. Each is a whole row sealed with the same key, so nothing is
    /// torn; at worst a refresh that lost the race is asked for again.
    Beside,
}

/// Decide where this command changes a secret, before it writes anything.
///
/// Turso is [`SecretRoute::Local`]. On Postgres no process without the key
/// touches the vault: when the owner row names a daemon of this machine,
/// that daemon takes the change ([`SecretRoute::Daemon`]; a standby refuses
/// with its own sentence, and a daemon without a sealed vault is refused);
/// when no instance owns the database the key is required, and the
/// database is claimed for the command, migrated and the vault installed
/// ([`SecretRoute::Owned`]). A daemon of this machine that does not own the
/// database (a Turso one, or one on another schema) is never asked. A
/// missing key, or a database another instance owns, is refused with a
/// sentence naming `kubectl exec` and the key variable, and nothing was
/// written.
pub async fn route_secret_command(cfg: &GlobalConfig) -> anyhow::Result<SecretRoute> {
    route(cfg, false, false).await
}

/// [`route_secret_command`] for a command that seals a secret (`connect
/// github`, a server `connect` and `disconnect`, `github app set` and
/// `remove`): a token or an App cannot be handed to a daemon over the
/// socket, so with a daemon of this machine owning the database the command
/// seals it itself, beside the daemon ([`SecretRoute::Beside`]). That needs
/// the key here too: without it, or with a key other than the one the rows
/// were sealed with, it is refused before anything is written.
pub async fn route_sealing_command(cfg: &GlobalConfig) -> anyhow::Result<SecretRoute> {
    route(cfg, true, false).await
}

/// [`route_sealing_command`] for a server sign-in (`connect <url>`,
/// `disconnect <name>`): never [`SecretRoute::Owned`]. The sign-in's record
/// (`sources.json`) lives in the state folder of the instance that serves
/// it and its credential in the database, so a command away from that
/// instance (on a free database) is refused before anyone signs in: server
/// sign-ins are made in the pod, beside its daemon.
pub async fn route_server_sign_in(cfg: &GlobalConfig) -> anyhow::Result<SecretRoute> {
    route(cfg, true, true).await
}

async fn route(
    cfg: &GlobalConfig,
    seals: bool,
    server_sign_in: bool,
) -> anyhow::Result<SecretRoute> {
    #[cfg(feature = "postgres")]
    if cfg.database().backend == crystalline_core::config::DatabaseBackend::Postgres {
        return route_on_postgres(cfg, seals, server_sign_in).await;
    }
    let _ = (cfg, seals, server_sign_in);
    Ok(SecretRoute::Local)
}

#[cfg(feature = "postgres")]
async fn route_on_postgres(
    cfg: &GlobalConfig,
    seals: bool,
    server_sign_in: bool,
) -> anyhow::Result<SecretRoute> {
    use crystalline_remote::sealed::SealKey;

    // No credential call of this process reaches the keychain or a file
    // from here on, whichever way it goes.
    crystalline_remote::vault::require();
    // Who owns this configuration's database decides, not whether a daemon
    // of this machine answers: that daemon may serve another database (a
    // Turso one, or another schema), and a change handed to it would land
    // in the wrong place.
    match holder(cfg).await {
        Holder::Here {
            owner: Some(row), ..
        } if row.command.is_none() => {
            if let Some(route) = route_beside_owner(cfg, seals, row.pid).await? {
                return Ok(route);
            }
        }
        Holder::Elsewhere { database, owner } => {
            let owned = DatabaseOwned {
                message: pg::command_refusal(&database, owner.as_ref(), &command_example()),
            };
            anyhow::bail!(pg::sealed_elsewhere(&owned));
        }
        // Who owns it could not be read: nothing can be routed safely.
        Holder::Unknown { database, error } => {
            anyhow::bail!(pg::secret_owner_unknown(&database, &error));
        }
        _ => {}
    }
    // No daemon of this machine owns it: the command claims the database
    // for itself, which a server sign-in may not do (its record lives in
    // the state folder of the instance that serves it).
    if server_sign_in {
        anyhow::bail!(pg::server_sign_in_in_the_pod(&command_example()));
    }
    match SealKey::from_env() {
        Ok(Some(_)) => {}
        Ok(None) => anyhow::bail!(pg::no_key_refusal(&command_example())),
        Err(e) => anyhow::bail!("{e}"),
    }
    let claim = match claim_database_for_command(cfg, &command_label()).await {
        Ok(Some(claim)) => claim,
        Ok(None) => anyhow::bail!("the database is not on Postgres"),
        Err(e) => match e.downcast_ref::<DatabaseOwned>() {
            Some(owned) => anyhow::bail!(pg::sealed_elsewhere(owned)),
            None => return Err(e),
        },
    };
    // The store open migrates the database (the sealed secrets' table
    // among it) and installs the vault with the key.
    drop(open_store_as(Some(&claim), cfg, None, false).await?);
    // The claim lives in this process's slot until `main` releases it
    // (`release_command_database`), so it holds for the whole command
    // whatever the caller does with this handle.
    Ok(SecretRoute::Owned(claim))
}

/// The route of a command while the owner row names a daemon of this
/// machine: the change goes to it ([`SecretRoute::Daemon`]) or is sealed
/// beside it ([`SecretRoute::Beside`]). `None` when no daemon answers on
/// this machine's control socket, or when the daemon that answers is not
/// the process the owner row names (`owner_pid`: the row of an earlier
/// daemon of this state folder, before the next owner announced itself):
/// the command then goes on as if there were none, and its claim is
/// refused while the lock is held.
///
/// A standby answers with its own refusal (the restart window, while the
/// row still names a predecessor of this state folder).
///
/// Only a daemon with a sealed vault takes either route: one without (a
/// Turso daemon, or one older than this command) cannot delete a sealed
/// row or say which key it seals with.
#[cfg(feature = "postgres")]
async fn route_beside_owner(
    cfg: &GlobalConfig,
    seals: bool,
    owner_pid: i64,
) -> anyhow::Result<Option<SecretRoute>> {
    use crystalline_remote::sealed::SealKey;
    use serde_json::Value;

    let holder = match crate::client::ctl_if_running(serde_json::json!({ "cmd": "holder" })).await {
        Ok(Some(holder)) => holder,
        Ok(None) => return Ok(None),
        Err(e) => anyhow::bail!(
            "the daemon of this machine did not answer ({e}), so nothing was changed; run the command again once it answers"
        ),
    };
    if let Some(refusal) = holder.get("standby").and_then(Value::as_str) {
        anyhow::bail!("{refusal}");
    }
    // Both pids are the daemon's own, so they compare inside a container
    // too.
    if holder.get("pid").and_then(Value::as_i64) != Some(owner_pid) {
        return Ok(None);
    }
    let Some(theirs) = holder.get("secret_key_id").and_then(Value::as_str) else {
        anyhow::bail!(pg::daemon_without_vault());
    };
    if !seals {
        return Ok(Some(SecretRoute::Daemon));
    }
    let key = match SealKey::from_env() {
        Ok(Some(key)) => key,
        Ok(None) => anyhow::bail!(pg::no_key_beside_daemon()),
        Err(e) => anyhow::bail!("{e}"),
    };
    // The daemon's key decides, also while no row is sealed yet: a row this
    // process sealed with another key would be one the daemon cannot open.
    let mine = key.key_id();
    if theirs != mine {
        anyhow::bail!(pg::other_key_than_daemon(theirs, &mine));
    }
    let target = crystalline_index::postgres::PgTarget::from_config(&cfg.database())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let database = target.describe();
    crate::secret_start::open_vault_beside_owner(target, &database, key).await?;
    Ok(Some(SecretRoute::Beside))
}

/// Refuse, before its first write, a daemonless command on Postgres that
/// will need a credential (an origin connect, update, share or withdraw
/// reaches GitHub) when this process has no secret key: its credential
/// calls would fail half way. Turso, and Postgres with a key, pass.
pub fn require_key_for_credentials(cfg: &GlobalConfig) -> anyhow::Result<()> {
    #[cfg(feature = "postgres")]
    if cfg.database().backend == crystalline_core::config::DatabaseBackend::Postgres {
        return match crystalline_remote::sealed::SealKey::from_env() {
            Ok(Some(_)) => Ok(()),
            Ok(None) => anyhow::bail!(pg::no_key_refusal(&command_example())),
            Err(e) => anyhow::bail!("{e}"),
        };
    }
    let _ = cfg;
    Ok(())
}

/// Whether `e` is the CLI rule's refusal to open the database: a
/// [`DatabaseOwned`] or a [`DatabaseNotOpened`]. Its sentence goes out as it
/// is, never inside the wording of an index that failed to open.
pub fn refused_by_cli_rule(e: &anyhow::Error) -> bool {
    e.is::<DatabaseOwned>() || e.is::<DatabaseNotOpened>()
}

/// The claim this process's command opens its stores under: a claim on
/// Postgres and nothing on Turso. A [`DatabaseOwned`] when another instance
/// owns the database. A read-only report on Postgres never opens it, free
/// or owned (only an owner migrates): it is refused with the owner's
/// sentence or a [`DatabaseNotOpened`].
pub(crate) async fn claim_for_this_command(
    cfg: &GlobalConfig,
) -> anyhow::Result<Option<CommandDbOwner>> {
    if is_report() {
        return report_on(cfg).await;
    }
    claim_database_for_command(cfg, &command_label()).await
}

#[cfg(not(feature = "postgres"))]
async fn report_on(_cfg: &GlobalConfig) -> anyhow::Result<Option<CommandDbOwner>> {
    Ok(None)
}

/// A read-only report: `Ok(None)` on Turso, where it opens the index as
/// before; on Postgres always a refusal that says why it opens nothing. A
/// look that failed or ran out of time counts as unavailable, never as free.
#[cfg(feature = "postgres")]
async fn report_on(cfg: &GlobalConfig) -> anyhow::Result<Option<CommandDbOwner>> {
    let example = command_example();
    match holder(cfg).await {
        Holder::NotPostgres => Ok(None),
        Holder::Free { database } => Err(anyhow::Error::new(DatabaseNotOpened {
            message: pg::no_active_instance(&database, &example),
        })),
        Holder::Here { database, owner } | Holder::Elsewhere { database, owner } => {
            Err(anyhow::Error::new(DatabaseOwned {
                message: pg::command_refusal(&database, owner.as_ref(), &example),
            }))
        }
        Holder::Unknown { database, error } => Err(anyhow::Error::new(DatabaseNotOpened {
            message: pg::owner_unknown(&database, &error, &example),
        })),
    }
}

/// `doctor --fix` on Postgres, before it changes anything: refused with the
/// owner's sentence (a [`DatabaseOwned`]) while an instance of another
/// machine owns the database. An instance of this machine is not refused:
/// repairing its daemon is what `--fix` is for. Turso, a free database and a
/// look that failed pass.
pub async fn refuse_fix_when_owned(cfg: &GlobalConfig) -> anyhow::Result<()> {
    #[cfg(feature = "postgres")]
    if let Holder::Elsewhere { database, owner } = holder(cfg).await {
        return Err(anyhow::Error::new(DatabaseOwned {
            message: pg::command_refusal(&database, owner.as_ref(), &command_example()),
        }));
    }
    #[cfg(not(feature = "postgres"))]
    let _ = cfg;
    Ok(())
}

/// Where the accounts of `cfg` live (spec section A): the `web-auth.db`
/// file in the state directory on Turso, the configured database on
/// Postgres.
pub(crate) fn accounts_location(cfg: &GlobalConfig) -> anyhow::Result<AccountsLocation> {
    let state_dir = crystalline_core::config::state_dir()
        .map_err(|e| anyhow::anyhow!("could not resolve the state directory: {e}"))?;
    Ok(AccountsLocation::resolve(&cfg.database(), &state_dir))
}

/// The refusal of a Postgres accounts location in a build without the
/// Postgres backend.
#[cfg(not(feature = "postgres"))]
fn no_postgres_backend() -> anyhow::Error {
    anyhow::anyhow!(
        "the accounts live in a Postgres database, and this build of Crystalline has no Postgres backend"
    )
}

/// Open the accounts store for a command that writes beside the instance
/// that owns the database (`users`, `domain members`, the account link of a
/// GitHub sign-in), by spec section L rule 4.
///
/// On Turso the `web-auth.db` file opens as it always did. On Postgres the
/// store opens in the role `cli`, with no fence and without migrating, so
/// it may run while a daemon owns the database. Accounts tables behind this
/// binary are migrated only by an owner: the database is claimed for the
/// command and migrated when it is free, and the command is refused naming
/// both versions when another instance holds it. Tables ahead of this
/// binary are refused as too new.
pub async fn open_accounts_for_cli(cfg: &GlobalConfig) -> anyhow::Result<AuthStore> {
    let loc = accounts_location(cfg)?;
    match &loc {
        AccountsLocation::File(path) => AuthStore::open(path).await,
        #[cfg(feature = "postgres")]
        AccountsLocation::Postgres(_) => match held_claim().await {
            // A command that claimed the database opens as its owner.
            Some(claim) => AuthStore::open_at(&loc, &claim.session_options()).await,
            None => open_cli_accounts_on_postgres(cfg, &loc).await,
        },
        #[cfg(not(feature = "postgres"))]
        AccountsLocation::Postgres(_) => Err(no_postgres_backend()),
    }
}

/// [`open_accounts_for_cli`] when this machine may already hold accounts,
/// `None` when it holds none: a Turso file that does not exist is not
/// created. A Postgres database always counts as holding them.
pub async fn existing_accounts_for_cli(cfg: &GlobalConfig) -> anyhow::Result<Option<AuthStore>> {
    if !accounts_location(cfg)?.exists() {
        return Ok(None);
    }
    open_accounts_for_cli(cfg).await.map(Some)
}

/// The Postgres arm of [`open_accounts_for_cli`].
#[cfg(feature = "postgres")]
async fn open_cli_accounts_on_postgres(
    cfg: &GlobalConfig,
    loc: &AccountsLocation,
) -> anyhow::Result<AuthStore> {
    use crystalline_identity::auth_store::AuthOpenError;
    use crystalline_index::postgres::{PgRole, PgSessionOptions};

    let opts = PgSessionOptions {
        instance_tag: crystalline_core::config::read_or_create_instance_id().ok(),
        fence: None,
    };
    let (found, need) = match AuthStore::open_at_no_migrate_as(loc, &opts, PgRole::Cli).await {
        Ok(store) => return Ok(store),
        Err(AuthOpenError::Behind { found, need }) => (found, need),
        Err(AuthOpenError::Other(e)) => return Err(e),
        Err(too_new) => return Err(anyhow::Error::new(too_new)),
    };
    // Only an owner migrates: take the database for this command when it
    // is free, and refuse when another instance has it.
    let claim = match claim_database_for_command(cfg, &command_label()).await {
        Ok(Some(claim)) => claim,
        Ok(None) => anyhow::bail!("the accounts database is not on Postgres"),
        Err(e) => match e.downcast_ref::<DatabaseOwned>() {
            Some(owned) => anyhow::bail!(
                "the accounts tables are at version {found}, this binary needs {need}, and only the instance that owns the database migrates them: {owned}"
            ),
            None => return Err(e),
        },
    };
    // The command owns the database now: it opens as the owner does, with
    // the claim's fenced options in the role `auth`, so a lost lock stops
    // the migration and the writes after it.
    AuthStore::open_at(loc, &claim.session_options()).await
}

/// Open the accounts store at `loc` for this process's own stack: the
/// `web-auth.db` file on Turso; on Postgres in the database this command
/// claimed (claimed here when it was not yet), with the claim's tag and
/// fence, so the store migrates as the owner it is.
pub(crate) async fn open_accounts_for_command(
    cfg: &GlobalConfig,
    loc: &AccountsLocation,
) -> anyhow::Result<AuthStore> {
    match loc {
        AccountsLocation::File(path) => AuthStore::open(path).await,
        #[cfg(feature = "postgres")]
        AccountsLocation::Postgres(_) => {
            let Some(claim) = claim_database_for_command(cfg, &command_label()).await? else {
                anyhow::bail!("the accounts database is not on Postgres");
            };
            AuthStore::open_at(loc, &claim.session_options()).await
        }
        #[cfg(not(feature = "postgres"))]
        AccountsLocation::Postgres(_) => {
            let _ = cfg;
            Err(no_postgres_backend())
        }
    }
}

/// A read-only report's refusal to open a Postgres database: there is no
/// active instance to read it through, or who owns it could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseNotOpened {
    /// The sentence: why nothing was opened.
    pub message: String,
}

impl std::fmt::Display for DatabaseNotOpened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DatabaseNotOpened {}

/// Who holds the Postgres database of a configuration, read without the
/// lock.
#[derive(Debug)]
#[cfg_attr(
    not(feature = "postgres"),
    allow(dead_code, reason = "only the postgres backend finds a holder")
)]
pub(crate) enum Holder {
    /// Turso, or a build without the Postgres backend.
    NotPostgres,
    /// No session holds the owner lock.
    Free { database: String },
    /// A process of this machine holds it: its daemon or one of its
    /// commands (the owner row carries this machine's instance id).
    #[cfg(feature = "postgres")]
    Here {
        database: String,
        owner: Option<crystalline_index::postgres::OwnerRow>,
    },
    /// Another machine holds it, or a holder that has not announced itself.
    #[cfg(feature = "postgres")]
    Elsewhere {
        database: String,
        owner: Option<crystalline_index::postgres::OwnerRow>,
    },
    /// The look failed or ran out of time.
    Unknown { database: String, error: String },
}

impl Holder {
    /// The sentence of a second instance started by mistake (spec section
    /// O), when another machine holds the database.
    pub(crate) fn not_served_here(&self) -> Option<String> {
        match self {
            #[cfg(feature = "postgres")]
            Holder::Elsewhere { database, owner } => {
                Some(pg::not_served_here(database, owner.as_ref()))
            }
            _ => None,
        }
    }

    /// What the database's owner row says when another machine's holder is
    /// named there, as a note beside another refusal: it only reports the
    /// row, and never says this machine does not serve the database (a
    /// daemon of this machine that is starting can hold the database while
    /// the row still names the instance before it).
    pub(crate) fn owner_row_note(&self) -> Option<String> {
        match self {
            #[cfg(feature = "postgres")]
            Holder::Elsewhere { database, owner } => {
                Some(pg::owner_row_note(database, owner.as_ref()))
            }
            _ => None,
        }
    }

    /// The command of this machine that holds the database, with its pid,
    /// when one does (not its daemon).
    pub(crate) fn local_command(&self) -> Option<(String, i64)> {
        match self {
            #[cfg(feature = "postgres")]
            Holder::Here {
                owner: Some(owner), ..
            } => owner.command.clone().map(|c| (c, owner.pid)),
            _ => None,
        }
    }
}

/// Who holds the Postgres database of `cfg`, read on a probe connection
/// without the lock, bounded in time.
#[cfg(not(feature = "postgres"))]
pub(crate) async fn holder(_cfg: &GlobalConfig) -> Holder {
    Holder::NotPostgres
}

/// Who holds the Postgres database of `cfg`, read on a probe connection
/// without the lock, bounded in time.
#[cfg(feature = "postgres")]
pub(crate) async fn holder(cfg: &GlobalConfig) -> Holder {
    use crystalline_core::config::DatabaseBackend;
    use crystalline_index::postgres::{PgTarget, probe_owner};

    let config = cfg.database();
    if config.backend != DatabaseBackend::Postgres {
        return Holder::NotPostgres;
    }
    let target = match PgTarget::from_config(&config) {
        Ok(target) => target,
        Err(e) => {
            return Holder::Unknown {
                database: "the configured database".to_string(),
                error: e.to_string(),
            };
        }
    };
    let database = target.describe();
    let looked = tokio::time::timeout(pg::LOOK_WAIT, probe_owner(&target)).await;
    match looked {
        Err(_) => Holder::Unknown {
            database,
            error: format!("no answer within {}s", pg::LOOK_WAIT.as_secs()),
        },
        Ok(Err(e)) => Holder::Unknown {
            database,
            error: e.to_string(),
        },
        Ok(Ok(None)) => Holder::Free { database },
        Ok(Ok(Some(owner))) => {
            let machine = crystalline_core::config::read_or_create_instance_id().ok();
            let here = owner
                .as_ref()
                .filter(|o| o.released_at.is_none())
                .zip(machine.as_deref())
                .is_some_and(|(o, m)| pg::of_this_machine(&o.instance_id, m));
            if here {
                Holder::Here { database, owner }
            } else {
                Holder::Elsewhere { database, owner }
            }
        }
    }
}

#[cfg(feature = "postgres")]
pub(crate) mod pg {
    use std::time::Duration;

    use crystalline_index::postgres::OwnerRow;

    /// How long a look at the owner may take before it is given up.
    pub(crate) const LOOK_WAIT: Duration = Duration::from_secs(5);

    /// A fresh id for this process: 16 random hex characters, then `-` and
    /// this machine's instance id when there is one. The random head is
    /// what tags its connections (their first 8 characters), so the
    /// takeover ends a paused daemon of this machine too; the tail tells a
    /// client of this machine that the holder is one of its own commands.
    pub(super) fn process_instance_id(machine: Option<&str>) -> String {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the OS CSPRNG is available");
        let head: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        match machine {
            Some(machine) => format!("{head}-{machine}"),
            None => head,
        }
    }

    /// Whether an owner row's `instance_id` is this machine's: its daemon
    /// (the machine id itself) or one of its commands (a process id ending
    /// in it).
    pub(crate) fn of_this_machine(instance_id: &str, machine: &str) -> bool {
        !machine.is_empty()
            && (instance_id == machine || instance_id.ends_with(&format!("-{machine}")))
    }

    /// The owner row, unless it records a release (then it is the instance
    /// before, and the holder has not announced itself yet).
    fn current(owner: Option<&OwnerRow>) -> Option<&OwnerRow> {
        owner.filter(|o| o.released_at.is_none())
    }

    fn since(owner: &OwnerRow) -> String {
        owner.since.rfc3339_secs_z()
    }

    /// `running `<command>`, ` when the holder is a command, else nothing.
    fn running(owner: &OwnerRow) -> String {
        owner
            .command
            .as_deref()
            .map(|c| format!("running `{c}`, "))
            .unwrap_or_default()
    }

    /// The refusal of a command on an owned database (spec section L).
    pub(crate) fn command_refusal(
        database: &str,
        owner: Option<&OwnerRow>,
        example: &str,
    ) -> String {
        let holder = match current(owner) {
            Some(o) => format!(
                "the active instance {} ({}pid {}, since {})",
                o.label,
                running(o),
                o.pid,
                since(o)
            ),
            None => "an instance that has not announced itself yet".to_string(),
        };
        format!(
            "the database {database} is owned by {holder}. Run the command there, for example `kubectl exec <pod> -- {example}`, or stop that instance first."
        )
    }

    /// The refusal of an autostarted daemon or an embedded MCP stack on an
    /// owned database (spec section O, a second instance started by
    /// mistake).
    pub(crate) fn not_served_here(database: &str, owner: Option<&OwnerRow>) -> String {
        let holder = match current(owner) {
            Some(o) => format!("{} ({}since {})", o.label, running(o), since(o)),
            None => "an instance that has not announced itself yet".to_string(),
        };
        format!(
            "the database {database} is owned by {holder}; this machine does not serve it. Point Crystalline at another database or schema, or stop that instance."
        )
    }

    /// The note of [`super::Holder::owner_row_note`].
    pub(crate) fn owner_row_note(database: &str, owner: Option<&OwnerRow>) -> String {
        match current(owner) {
            Some(o) => format!(
                "the database {database} is held, and its owner row names {} ({}since {}), not this machine",
                o.label,
                running(o),
                since(o)
            ),
            None => format!(
                "the database {database} is held by an instance that has not announced itself yet"
            ),
        }
    }

    /// A read-only report on a database no instance owns: it opens nothing,
    /// because only an owner migrates.
    pub(crate) fn no_active_instance(database: &str, example: &str) -> String {
        format!(
            "the database {database} has no active instance, and `{example}` does not open it: only the instance that owns a database opens it. Start one with `crystalline serve`."
        )
    }

    /// The refusal of a command that changes a secret on Postgres without
    /// the key and with no daemon of this machine to take the change.
    pub(crate) fn no_key_refusal(example: &str) -> String {
        format!(
            "this command reads or changes a GitHub token or a server sign-in, and on Postgres they are sealed in the database with the instance's secret key, which this process does not have, so nothing was changed. Run the command on the active instance, for example `kubectl exec <pod> -- {example}`, or set {} (or {}) to the instance's key",
            crystalline_remote::sealed::KEY_ENV,
            crystalline_remote::sealed::KEY_FILE_ENV
        )
    }

    /// The refusal of a command that seals a secret beside the daemon of
    /// its machine without the key.
    pub(crate) fn no_key_beside_daemon() -> String {
        format!(
            "this command seals a GitHub token, a GitHub App or a server sign-in in the database itself, and this process has no secret key, so nothing was changed. Set {} (or {}) to the key of the active instance and run the command again; inside the instance's container it is set already",
            crystalline_remote::sealed::KEY_ENV,
            crystalline_remote::sealed::KEY_FILE_ENV
        )
    }

    /// The refusal of a command whose database is owned by the daemon of its
    /// machine, when that daemon has no sealed vault to change the secret
    /// in.
    pub(crate) fn daemon_without_vault() -> String {
        "the daemon of this machine has no sealed vault, so nothing was changed: it is not the instance that owns this database, or it is older than this command. Run the command where the instance that owns the database runs, or update this machine's daemon and run it again".to_string()
    }

    /// The refusal of a command that changes a secret while who owns its
    /// database could not be read.
    pub(crate) fn secret_owner_unknown(database: &str, error: &str) -> String {
        format!(
            "could not check who owns the database {database} ({error}), so nothing was changed; run the command again once the database answers"
        )
    }

    /// The refusal of a server sign-in (`connect <url>`, `disconnect`) on
    /// Postgres away from the instance that serves it.
    pub(crate) fn server_sign_in_in_the_pod(example: &str) -> String {
        format!(
            "a server sign-in on Postgres is made in the pod, where the daemon keeps sources.json and the sealed vault together, so nothing was changed. Run the command there, for example `kubectl exec <pod> -- {example}`"
        )
    }

    /// The refusal of a command whose key is not the one the daemon of its
    /// machine seals with.
    pub(crate) fn other_key_than_daemon(theirs: &str, mine: &str) -> String {
        format!(
            "the active instance on this machine seals its secrets with key {theirs}, but {var} here is key {mine}, so nothing was changed. Set {var} to the instance's key",
            var = crate::secret_start::key_variable_in_use()
        )
    }

    /// The refusal of a command that changes a secret on a database another
    /// instance owns.
    pub(crate) fn sealed_elsewhere(owned: &super::DatabaseOwned) -> String {
        format!(
            "the GitHub tokens and server sign-ins of this instance are sealed in the database with {}, and only the instance that owns the database can change them, so nothing was changed: {owned}",
            crystalline_remote::sealed::KEY_ENV
        )
    }

    /// A read-only report that could not read who owns the database.
    pub(crate) fn owner_unknown(database: &str, error: &str, example: &str) -> String {
        format!(
            "could not read who owns the database {database} ({error}), and `{example}` does not open it without the instance that owns it."
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn row(command: Option<&str>, released: bool) -> OwnerRow {
            OwnerRow {
                instance_id: "0123456789abcdef".to_string(),
                label: "prod-a".to_string(),
                command: command.map(str::to_string),
                pid: 42,
                backend_pid: 4242,
                version: "0.33.0".to_string(),
                since: crystalline_core::time::Stamp::from_ymd_hms(2026, 11, 16, 9, 30, 0).unwrap(),
                released_at: released.then(|| {
                    crystalline_core::time::Stamp::from_ymd_hms(2026, 11, 16, 10, 0, 0).unwrap()
                }),
            }
        }

        /// A daemon of this machine without a sealed vault cannot take a
        /// secret change: the refusal says nothing was changed and why,
        /// and no longer blames only the daemon's age.
        #[test]
        fn a_daemon_without_a_vault_is_refused_with_both_reasons() {
            let said = daemon_without_vault();
            assert!(
                said.starts_with(
                    "the daemon of this machine has no sealed vault, so nothing was changed"
                ),
                "{said}"
            );
            assert!(
                said.contains("it is not the instance that owns this database"),
                "{said}"
            );
            assert!(said.contains("or it is older than this command"), "{said}");
        }

        /// The command refusal names the owner, the command it runs if any,
        /// and the way to run this command on it.
        #[test]
        fn the_command_refusal_names_the_owner_and_kubectl_exec() {
            assert_eq!(
                command_refusal(
                    "db:5432/c, schema a",
                    Some(&row(None, false)),
                    "crystalline reindex"
                ),
                "the database db:5432/c, schema a is owned by the active instance prod-a (pid 42, since 2026-11-16T09:30:00Z). Run the command there, for example `kubectl exec <pod> -- crystalline reindex`, or stop that instance first."
            );
            assert!(
                command_refusal(
                    "x",
                    Some(&row(Some("crystalline sync"), false)),
                    "crystalline reindex"
                )
                .contains("prod-a (running `crystalline sync`, pid 42,")
            );
            for unknown in [None, Some(row(None, true))] {
                assert!(
                    command_refusal("x", unknown.as_ref(), "crystalline reindex")
                        .contains("owned by an instance that has not announced itself yet.")
                );
            }
        }

        /// The sentence of a second instance started by mistake, as the
        /// spec words it.
        #[test]
        fn the_autostart_refusal_says_this_machine_does_not_serve_it() {
            assert_eq!(
                not_served_here("db:5432/c, schema a", Some(&row(None, false))),
                "the database db:5432/c, schema a is owned by prod-a (since 2026-11-16T09:30:00Z); this machine does not serve it. Point Crystalline at another database or schema, or stop that instance."
            );
        }

        /// A command holder is named with its command in both sentences.
        #[test]
        fn both_sentences_name_a_command_holder() {
            let held = row(Some("crystalline sync"), false);
            assert!(
                not_served_here("x", Some(&held))
                    .contains("owned by prod-a (running `crystalline sync`, since ")
            );
        }

        /// The note beside an unknown local holder reports the owner row
        /// and does not claim this machine cannot serve the database.
        #[test]
        fn the_owner_row_note_reports_the_row() {
            let held = row(Some("crystalline sync"), false);
            let note = owner_row_note("db", Some(&held));
            assert!(
                note.starts_with(
                    "the database db is held, and its owner row names prod-a (running `crystalline sync`, since "
                ),
                "{note}"
            );
            assert!(note.ends_with(", not this machine"), "{note}");
            assert!(!note.contains("does not serve"), "{note}");
            assert_eq!(
                owner_row_note("db", None),
                "the database db is held by an instance that has not announced itself yet"
            );
        }

        /// A process id is fresh per process, its first 8 characters tag it
        /// apart from any other, and its tail says which machine runs it.
        #[test]
        fn a_process_instance_id_is_random_and_names_its_machine() {
            let machine = "5f0c2a7e-0000-4000-8000-000000000001";
            let (a, b) = (
                process_instance_id(Some(machine)),
                process_instance_id(Some(machine)),
            );
            assert_ne!(a[..8], b[..8]);
            assert!(a[..16].bytes().all(|c| c.is_ascii_hexdigit()));
            assert!(of_this_machine(&a, machine));
            assert!(of_this_machine(machine, machine), "the daemon's own id");
            assert!(!of_this_machine(&a, "another-machine"));
            assert!(!of_this_machine("0123456789abcdef", ""));
            assert_eq!(process_instance_id(None).len(), 16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `--config` of a command is read in both spellings, the last one
    /// winning; without one there is none.
    #[test]
    fn the_config_flag_is_read_from_the_words() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            config_flag(&words(&["serve", "--config", "/a.yaml"])),
            Some(std::path::PathBuf::from("/a.yaml"))
        );
        assert_eq!(
            config_flag(&words(&["--config=/a.yaml", "sync", "--config", "/b.yaml"])),
            Some(std::path::PathBuf::from("/b.yaml"))
        );
        assert_eq!(config_flag(&words(&["status", "--json"])), None);
        // After `--` every word is a value: content that reads like the flag
        // does not move the command to another configuration.
        assert_eq!(
            config_flag(&words(&[
                "write", "eng", "Note", "--", "--config", "/b.yaml"
            ])),
            None
        );
        assert_eq!(
            config_flag(&words(&[
                "--config",
                "/a.yaml",
                "write",
                "eng",
                "--",
                "--config=/b.yaml"
            ])),
            Some(std::path::PathBuf::from("/a.yaml"))
        );
    }

    /// The example for another machine leaves this machine's paths out.
    #[test]
    fn the_example_drops_local_paths() {
        let words: Vec<String> = [
            "--db",
            "/tmp/x.db",
            "reindex",
            "--full",
            "--config=/home/me/c.yaml",
            "--config",
            "/c.yaml",
        ]
        .iter()
        .map(|w| w.to_string())
        .collect();
        assert_eq!(without_local_paths(&words), ["reindex", "--full"]);
    }

    /// The example never repeats the value of a flag that carries a secret,
    /// in either spelling; other flags and their values stay.
    #[test]
    fn the_example_hides_secret_values() {
        let words: Vec<String> = [
            "connect",
            "github",
            "--token",
            "ghp_secret1",
            "--client-secret=hush",
            "--mcp-token",
            "label-a",
            "--private-key",
            "-----BEGIN",
            "--password=pw",
            "--key",
            "k1",
            "--full",
            "--name",
            "alpha",
            "--token-file",
            "/t",
        ]
        .iter()
        .map(|w| w.to_string())
        .collect();
        assert_eq!(
            without_secret_values(&words),
            [
                "connect",
                "github",
                "--token",
                "<value>",
                "--client-secret=<value>",
                "--mcp-token",
                "<value>",
                "--private-key",
                "<value>",
                "--password=<value>",
                "--key",
                "k1",
                "--full",
                "--name",
                "alpha",
                "--token-file",
                "/t",
            ]
        );
    }
}
