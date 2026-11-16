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

use crystalline_core::config::GlobalConfig;
use crystalline_index::Store;
use tokio::sync::Mutex as TokioMutex;

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
/// out of an example meant for another machine.
fn command_example() -> String {
    match COMMAND.get() {
        Some(c) if !c.words.is_empty() => {
            format!("crystalline {}", without_local_paths(&c.words).join(" "))
        }
        Some(c) => c.label.clone(),
        None => "crystalline <command>".to_string(),
    }
}

/// `words` without `--db`, `--config` and their values (as `--db x` or
/// `--db=x`).
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
}

impl CommandDbOwner {
    /// The settings of every Postgres connection the command opens: tagged
    /// with its instance id and fenced on the held lock.
    #[cfg(feature = "postgres")]
    pub fn session_options(&self) -> crystalline_index::postgres::PgSessionOptions {
        crate::standby::owner_session_options(&self.inner.instance_id, self.inner.owner.fence())
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
    drop(held);
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
        }),
    };
    *slot = Some(held.clone());
    Ok(Claim::Held(held))
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
        owner
            .since
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
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

    /// A read-only report on a database no instance owns: it opens nothing,
    /// because only an owner migrates.
    pub(crate) fn no_active_instance(database: &str, example: &str) -> String {
        format!(
            "the database {database} has no active instance, and `{example}` does not open it: only the instance that owns a database opens it. Start one with `crystalline serve`."
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
        use chrono::{TimeZone, Utc};

        fn row(command: Option<&str>, released: bool) -> OwnerRow {
            OwnerRow {
                instance_id: "0123456789abcdef".to_string(),
                label: "prod-a".to_string(),
                command: command.map(str::to_string),
                pid: 42,
                backend_pid: 4242,
                version: "0.33.0".to_string(),
                since: Utc.with_ymd_and_hms(2026, 11, 16, 9, 30, 0).unwrap(),
                released_at: released
                    .then(|| Utc.with_ymd_and_hms(2026, 11, 16, 10, 0, 0).unwrap()),
            }
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
}
