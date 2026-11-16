//! The one way a Postgres connection is opened: the index pool, the read
//! pool, the auth arm, the lock connection, the side channel, the standby and
//! the status probes all go through [`connect_options`] and either
//! [`connect_one`] or a pool whose `after_connect` runs [`prepare_session`].
//!
//! Every connection carries an `application_name` that names the database
//! owner key, the instance and the role ([`application_name`]), and TCP
//! keepalives as startup options. The lock connection also gets an
//! `idle_session_timeout` and a `statement_timeout`, set at connect through
//! startup options, because a `SET LOCAL` outside a transaction does nothing.
//!
//! When the options carry an [`OwnerFence`], every connection in the roles
//! `pool`, `read`, `auth` and `side` checks at connect that the recorded lock
//! session still holds the owner lock ([`HOLDS_KEY_SQL`]) and fails the
//! connect when it does not. sqlx reconnects a pool transparently, so the
//! check runs in the pool's `after_connect`: a reconnect cannot bypass it.

use std::str::FromStr;

use crystalline_core::config::{DatabaseBackend, DatabaseConfig};
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection};

use crate::error::{IndexError, Result};

/// The advisory lock id of `name` in `schema`: FNV-1a 64 over the schema
/// name, a NUL and the name, computed here so every instance and every build
/// agrees on it. Advisory locks belong to the database, not to a schema, so
/// the schema is part of the id: two schemas on one database (the test
/// suites, or two separate teams) never shut each other out.
pub fn lock_key(schema: Option<&str>, name: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    let schema = schema.unwrap_or_default().as_bytes();
    for b in schema.iter().chain(&[0u8]).chain(name.as_bytes()) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h as i64
}

/// The key of the lock the active instance holds on `schema`.
pub fn owner_key(schema: Option<&str>) -> i64 {
    lock_key(schema, "instance-owner")
}

/// What a connection is for. It is the last part of its `application_name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PgRole {
    /// The owner's lock connection.
    Lock,
    /// The index store's pool.
    Pool,
    /// The index store's read connections.
    Read,
    /// The auth store's connections.
    Auth,
    /// The owner's side channel.
    Side,
    /// A standby waiting for the lock.
    Standby,
    /// A CLI command.
    Cli,
    /// A status probe.
    Probe,
}

impl PgRole {
    /// The role as it appears in `application_name`.
    pub fn as_str(&self) -> &'static str {
        match self {
            PgRole::Lock => "lock",
            PgRole::Pool => "pool",
            PgRole::Read => "read",
            PgRole::Auth => "auth",
            PgRole::Side => "side",
            PgRole::Standby => "standby",
            PgRole::Cli => "cli",
            PgRole::Probe => "probe",
        }
    }

    /// Whether a connection in this role belongs to the owner's process and
    /// so runs the fence check when the options carry a fence.
    fn is_fenced(self) -> bool {
        matches!(
            self,
            PgRole::Pool | PgRole::Read | PgRole::Auth | PgRole::Side
        )
    }
}

/// The tag a connection carries when its options name no instance.
const NO_INSTANCE_TAG: &str = "none";

/// `crystalline/<16 hex of owner_key>/<first 8 characters of instance_tag>/<role>`.
///
/// Postgres cuts `application_name` at 63 bytes, and a match on a cut name
/// would silently miss, so only the first 8 characters of the tag (the first
/// group of a UUID) are used. The longest result is 45 bytes. The owner key
/// already mixes in the schema, so the name stays short for any schema.
pub fn application_name(owner_key: i64, instance_tag: &str, role: PgRole) -> String {
    format!(
        "{}{}/{}",
        application_prefix(owner_key),
        short_tag(instance_tag),
        role.as_str()
    )
}

/// `crystalline/<16 hex of owner_key>/`: what every connection to the
/// database of `owner_key` carries at the start of its `application_name`.
pub(crate) fn application_prefix(owner_key: i64) -> String {
    format!("crystalline/{:016x}/", owner_key as u64)
}

/// The part of `instance_tag` that goes into `application_name`: its first 8
/// characters.
pub(crate) fn short_tag(instance_tag: &str) -> String {
    instance_tag.chars().take(8).collect()
}

/// What a connection of the owner's process checks at connect: the lock
/// session `lock_pid` still holds the owner lock `key`. A plain value, never
/// a handle to the owner.
#[derive(Clone, Copy, Debug)]
pub struct OwnerFence {
    /// The owner lock key, [`owner_key`] of the schema.
    pub key: i64,
    /// The backend pid of the session that holds the lock.
    pub lock_pid: i32,
}

/// The per-process settings every connection is built with. The default
/// names no instance and carries no fence, which is how every connection was
/// opened before ownership existed.
#[derive(Clone, Default)]
pub struct PgSessionOptions {
    /// The instance id, of which the first 8 characters go into
    /// `application_name`.
    pub instance_tag: Option<String>,
    /// The fence the owner-process roles check at connect.
    pub fence: Option<OwnerFence>,
}

/// Where a connection goes: the database URL and the schema every
/// connection routes to, if one is configured.
///
/// Its `Debug` shows only the host, port and database of the URL, never the
/// credentials or the query string.
#[derive(Clone)]
pub struct PgTarget {
    /// The `postgres://` URL.
    pub url: String,
    /// The configured schema; `None` uses the database default.
    pub schema: Option<String>,
}

impl std::fmt::Debug for PgTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgTarget")
            .field("url", &super::sanitize_url(&self.url))
            .field("schema", &self.schema)
            .finish()
    }
}

impl PgTarget {
    /// The target of a Postgres database block. Refuses a block that does not
    /// pass validation, names another backend or has no URL.
    pub fn from_config(cfg: &DatabaseConfig) -> Result<PgTarget> {
        cfg.validate()
            .map_err(|e| IndexError::Invalid(e.to_string()))?;
        if cfg.backend != DatabaseBackend::Postgres {
            return Err(IndexError::Invalid(
                "the database block does not name the postgres backend".to_string(),
            ));
        }
        let url = cfg.url.clone().ok_or_else(|| {
            IndexError::Invalid("the postgres backend requires a url".to_string())
        })?;
        Ok(PgTarget {
            url,
            schema: cfg.schema.clone(),
        })
    }
}

/// The TCP keepalive startup options every connection carries: the first
/// probe after 10 s idle, then every 5 s, and the session is given up after 3
/// unanswered probes.
const KEEPALIVES: [(&str, &str); 3] = [
    ("tcp_keepalives_idle", "10"),
    ("tcp_keepalives_interval", "5"),
    ("tcp_keepalives_count", "3"),
];

/// The connect options of a `role` connection to `t`: the URL's own options,
/// then the `application_name`, the keepalives and, for [`PgRole::Lock`], the
/// idle session and statement timeouts at the current owner timings
/// ([`super::owner::lock_session_settings`]).
pub(crate) fn connect_options(
    t: &PgTarget,
    role: PgRole,
    o: &PgSessionOptions,
) -> Result<PgConnectOptions> {
    let tag = o.instance_tag.as_deref().unwrap_or(NO_INSTANCE_TAG);
    let name = application_name(owner_key(t.schema.as_deref()), tag, role);
    let mut opts = PgConnectOptions::from_str(&t.url)
        .map_err(IndexError::from)?
        .application_name(&name)
        .options(KEEPALIVES);
    if role == PgRole::Lock {
        opts = opts.options(super::owner::lock_session_settings(
            &super::owner::OwnerTimings::current(),
        ));
    }
    Ok(opts)
}

/// Whether the session `$2` holds the advisory lock `$1` (a bigint key).
///
/// `pg_locks` shows a bigint key as two unsigned 32-bit halves, `classid`
/// the high one and `objid` the low one, with `objsubid = 1`. `>>` on a
/// bigint is arithmetic, so a key with the high bit set shifts in ones; the
/// mask keeps only the 32 bits the column holds.
pub(crate) const HOLDS_KEY_SQL: &str = "SELECT EXISTS (SELECT 1 FROM pg_locks \
     WHERE locktype = 'advisory' AND pid = $2 AND granted \
     AND classid::bigint = (($1 >> 32) & 4294967295) \
     AND objid::bigint = ($1 & 4294967295) AND objsubid = 1)";

/// The error a connect gets when the fence check finds the lock gone.
fn fenced_message(fence: &OwnerFence) -> String {
    format!(
        "this instance no longer owns the database: the lock session {} does not hold the owner lock",
        fence.lock_pid
    )
}

/// Ready a fresh session: route it to `schema` and, for an owner-process
/// role with a fence, check that the lock session still holds the lock.
///
/// Shared by [`connect_one`] and the pools' `after_connect`, so a pool's
/// reconnect runs the same check.
pub(crate) async fn prepare_session(
    conn: &mut PgConnection,
    schema: Option<&str>,
    role: PgRole,
    fence: Option<OwnerFence>,
) -> std::result::Result<(), sqlx::Error> {
    if let Some(s) = schema {
        // Only the search path: a schema that does not exist yet is accepted
        // and takes effect once it does.
        let setup = format!("SET search_path TO {}, public", super::quote_ident(s));
        sqlx::raw_sql(AssertSqlSafe(setup))
            .execute(&mut *conn)
            .await?;
    }
    if let Some(fence) = fence.filter(|_| role.is_fenced()) {
        let held: bool = sqlx::query_scalar(HOLDS_KEY_SQL)
            .bind(fence.key)
            .bind(fence.lock_pid)
            .fetch_one(&mut *conn)
            .await?;
        if !held {
            return Err(sqlx::Error::Protocol(fenced_message(&fence)));
        }
    }
    Ok(())
}

/// Open one bare `role` connection to `t`, routed to its schema and, for an
/// owner-process role with a fence, refused when the lock is gone.
pub async fn connect_one(t: &PgTarget, role: PgRole, o: &PgSessionOptions) -> Result<PgConnection> {
    let opts = connect_options(t, role, o)?;
    let mut conn = PgConnection::connect_with(&opts)
        .await
        .map_err(IndexError::from)?;
    if let Err(e) = prepare_session(&mut conn, t.schema.as_deref(), role, o.fence).await {
        let _ = conn.close().await;
        return Err(e.into());
    }
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postgres::{PostgresStore, ScratchSchema};

    fn pg_url() -> Option<String> {
        std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
            .ok()
            .filter(|u| !u.is_empty())
    }

    const ROLES: [PgRole; 8] = [
        PgRole::Lock,
        PgRole::Pool,
        PgRole::Read,
        PgRole::Auth,
        PgRole::Side,
        PgRole::Standby,
        PgRole::Cli,
        PgRole::Probe,
    ];

    /// The longest schema and a full UUID tag still fit in the 63 bytes
    /// Postgres keeps, in every role.
    #[test]
    fn application_name_fits_in_63_bytes() {
        let schema = "s".repeat(63);
        let key = owner_key(Some(&schema));
        let tag = "123e4567-e89b-12d3-a456-426614174000";
        for role in ROLES {
            let name = application_name(key, tag, role);
            assert!(name.len() <= 63, "{name} is {} bytes", name.len());
            assert!(name.starts_with(&format!("crystalline/{:016x}/123e4567/", key as u64)));
            assert!(name.ends_with(role.as_str()));
        }
        assert_eq!(
            application_name(-1, "ab", PgRole::Pool),
            "crystalline/ffffffffffffffff/ab/pool"
        );
    }

    /// The keys are fixed: two instances of different builds on one database
    /// must agree on them, so they never move with a Rust release or a
    /// process. The schema is part of them.
    #[test]
    fn lock_key_is_stable_and_schema_mixed() {
        // The work claim id from before the rename: live claims keep their key.
        assert_eq!(
            lock_key(None, "contradictions:1"),
            8_733_647_130_574_241_276
        );
        assert_eq!(owner_key(Some("team_a")), 1_436_526_938_115_825_605);
        assert_eq!(
            lock_key(Some(""), "contradictions:1"),
            lock_key(None, "contradictions:1")
        );
        assert_ne!(owner_key(Some("a")), owner_key(Some("b")));
        assert_ne!(
            lock_key(Some("a"), "contradictions:1"),
            lock_key(Some("a"), "contradictions:2")
        );
        assert_eq!(owner_key(Some("a")), lock_key(Some("a"), "instance-owner"));
    }

    /// A target is built from a validated Postgres block only.
    #[test]
    fn a_target_comes_from_a_postgres_block() {
        let cfg = DatabaseConfig {
            backend: DatabaseBackend::Postgres,
            url: Some("postgres://u@h/db".to_string()),
            schema: Some("team_a".to_string()),
        };
        let t = PgTarget::from_config(&cfg).unwrap();
        assert_eq!(t.url, "postgres://u@h/db");
        assert_eq!(t.schema.as_deref(), Some("team_a"));
        assert!(PgTarget::from_config(&DatabaseConfig::default()).is_err());
        let bad = DatabaseConfig {
            schema: Some("Team-A".to_string()),
            ..cfg
        };
        assert!(PgTarget::from_config(&bad).is_err());
    }

    /// A target's `Debug` shows where it goes and never the password or the
    /// query string.
    #[test]
    fn target_debug_hides_the_credentials() {
        let t = PgTarget {
            url: "postgres://alice:s3cret@db.example:5432/crys?password=other".to_string(),
            schema: Some("team_a".to_string()),
        };
        let shown = format!("{:?}", t.clone());
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(!shown.contains("other"), "{shown}");
        assert!(!shown.contains("alice"), "{shown}");
        assert!(shown.contains("db.example:5432/crys"), "{shown}");
        assert!(shown.contains("team_a"), "{shown}");
    }

    /// The application name and the keepalives go into every role's connect
    /// options, the URL's own options are kept, and only the lock connection
    /// gets the timeouts.
    #[test]
    fn connect_options_carry_the_name_keepalives_and_lock_timeouts() {
        let t = PgTarget {
            url: "postgres://u@h/db?options=-c%20work_mem%3D4MB".to_string(),
            schema: Some("team_a".to_string()),
        };
        let o = PgSessionOptions {
            instance_tag: Some("0123456789abcdef".to_string()),
            fence: None,
        };
        let pool = connect_options(&t, PgRole::Pool, &o).unwrap();
        assert_eq!(
            pool.get_application_name(),
            Some(application_name(owner_key(Some("team_a")), "01234567", PgRole::Pool).as_str())
        );
        let pool_opts = pool.get_options().unwrap();
        assert!(pool_opts.contains("work_mem"), "{pool_opts}");
        assert!(pool_opts.contains("tcp_keepalives_idle=10"), "{pool_opts}");
        assert!(
            pool_opts.contains("tcp_keepalives_interval=5"),
            "{pool_opts}"
        );
        assert!(pool_opts.contains("tcp_keepalives_count=3"), "{pool_opts}");
        assert!(!pool_opts.contains("statement_timeout"), "{pool_opts}");
        let lock = connect_options(&t, PgRole::Lock, &o).unwrap();
        let lock_opts = lock.get_options().unwrap();
        assert!(
            lock_opts.contains("idle_session_timeout=15s"),
            "{lock_opts}"
        );
        assert!(lock_opts.contains("statement_timeout=3s"), "{lock_opts}");
        let untagged = connect_options(&t, PgRole::Cli, &PgSessionOptions::default()).unwrap();
        assert!(
            untagged
                .get_application_name()
                .unwrap()
                .ends_with("/none/cli")
        );
    }

    /// A key with the high bit set in both halves is found by the sign-safe
    /// split.
    #[tokio::test]
    async fn holds_key_matches_a_negative_key() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "hkn");
        let key = (0x8000_0001_8000_0001_u64
            ^ lock_key(Some(scratch.name()), "neg") as u64 & 0x7fff_fffe_7fff_fffe)
            as i64;
        assert!(key < 0);
        let t = PgTarget {
            url: url.clone(),
            schema: None,
        };
        let mut conn = connect_one(&t, PgRole::Lock, &PgSessionOptions::default())
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        let before: bool = sqlx::query_scalar(HOLDS_KEY_SQL)
            .bind(key)
            .bind(pid)
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert!(!before);
        let taken: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert!(taken);
        let held: bool = sqlx::query_scalar(HOLDS_KEY_SQL)
            .bind(key)
            .bind(pid)
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert!(held);
        conn.close().await.unwrap();
    }

    /// The pool and the read connections carry the instance tag in
    /// `application_name`.
    #[tokio::test]
    async fn sessions_carry_application_name() {
        use crate::reader::ReaderBackend;
        use crate::store::ReadStore;
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "san");
        let tag = format!("{:08x}", lock_key(Some(scratch.name()), "tag") as u32);
        let opts = PgSessionOptions {
            instance_tag: Some(tag.clone()),
            fence: None,
        };
        let store = PostgresStore::build_with(&url, Some(scratch.name().to_string()), 1, &opts)
            .await
            .unwrap();
        store.domain_names().await.unwrap();
        let readers = store.readers.clone().unwrap();
        let lease = readers.lease().await.unwrap();
        lease.store().open_read().await.unwrap();
        let key = owner_key(Some(scratch.name()));
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT application_name FROM pg_stat_activity \
             WHERE starts_with(application_name, $1) ORDER BY 1",
        )
        .bind(format!("crystalline/{:016x}/{tag}/", key as u64))
        .fetch_all(&store.pool)
        .await
        .unwrap();
        lease.store().close_read(true).await;
        for role in [PgRole::Pool, PgRole::Read] {
            let want = application_name(key, &tag, role);
            assert!(names.contains(&want), "{want} not in {names:?}");
        }
    }

    /// A one-connection pool with the store pool's session setup and a
    /// one-second acquire timeout.
    fn short_pool(t: &PgTarget, o: &PgSessionOptions) -> sqlx::PgPool {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(1));
        crate::postgres::with_session_setup(pool, t.schema.clone(), PgRole::Pool, o.fence, None)
            .connect_lazy_with(connect_options(t, PgRole::Pool, o).unwrap())
    }

    /// Take the owner lock of `schema` on a lock connection of its own and
    /// return it with its fence.
    async fn hold_owner_lock(url: &str, schema: &str) -> (PgConnection, OwnerFence) {
        let t = PgTarget {
            url: url.to_string(),
            schema: Some(schema.to_string()),
        };
        let mut lock = connect_one(&t, PgRole::Lock, &PgSessionOptions::default())
            .await
            .unwrap();
        let key = owner_key(Some(schema));
        let taken: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut lock)
            .await
            .unwrap();
        assert!(taken);
        let lock_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut lock)
            .await
            .unwrap();
        (lock, OwnerFence { key, lock_pid })
    }

    /// A fenced connect fails when the recorded lock session does not hold
    /// the lock: a bare connection, a pool's connections (so a reconnect
    /// cannot bypass it) and a store's whole open. The roles outside the
    /// owner's process do not check.
    #[tokio::test]
    async fn fence_check_refuses_a_connect_when_the_lock_pid_does_not_hold_it() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "fcr");
        let (lock, fence) = hold_owner_lock(&url, scratch.name()).await;
        // The lock is held, but by another session than the one recorded.
        let wrong = OwnerFence {
            key: fence.key,
            lock_pid: fence.lock_pid.wrapping_add(1_000_000),
        };
        let t = PgTarget {
            url: url.clone(),
            schema: Some(scratch.name().to_string()),
        };
        let o = PgSessionOptions {
            instance_tag: Some("fcr".to_string()),
            fence: Some(wrong),
        };
        for role in [PgRole::Pool, PgRole::Read, PgRole::Auth, PgRole::Side] {
            let Err(err) = connect_one(&t, role, &o).await else {
                panic!("a {} connect went ahead", role.as_str());
            };
            assert!(err.to_string().contains("no longer owns"), "{err}");
        }
        for role in [PgRole::Lock, PgRole::Standby, PgRole::Cli, PgRole::Probe] {
            connect_one(&t, role, &o)
                .await
                .unwrap()
                .close()
                .await
                .unwrap();
        }
        // A pool's connect retries a failed `after_connect` until its acquire
        // timeout, so a refused pool connection surfaces as a time-out: the
        // store's own pool after its 30 s, a short test pool at once.
        let Err(err) = short_pool(&t, &o).acquire().await else {
            panic!("a pool connection went ahead");
        };
        assert!(matches!(err, sqlx::Error::PoolTimedOut), "{err}");
        let refused = PostgresStore::build_with(&url, t.schema.clone(), 0, &o).await;
        let Err(err) = refused else {
            panic!("the store open went ahead");
        };
        assert!(err.to_string().contains("timed out"), "{err}");
        // The right pid, after the lock session is gone.
        drop(lock);
        let gone = PgSessionOptions {
            instance_tag: Some("fcr".to_string()),
            fence: Some(fence),
        };
        let mut refused = false;
        for _ in 0..50 {
            if connect_one(&t, PgRole::Pool, &gone).await.is_err() {
                refused = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(refused, "a connect went ahead after the lock session ended");
    }

    /// A fenced connect goes ahead while the recorded session holds the lock,
    /// for a bare connection and a store's pools, and is refused again once
    /// the lock is released.
    #[tokio::test]
    async fn fence_check_admits_when_held() {
        use crate::store::Store;
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let scratch = ScratchSchema::new(&url, "fca");
        let (mut lock, fence) = hold_owner_lock(&url, scratch.name()).await;
        let t = PgTarget {
            url: url.clone(),
            schema: Some(scratch.name().to_string()),
        };
        let o = PgSessionOptions {
            instance_tag: Some("fca".to_string()),
            fence: Some(fence),
        };
        connect_one(&t, PgRole::Pool, &o)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        let store = PostgresStore::build_with(&url, t.schema.clone(), 1, &o)
            .await
            .unwrap();
        store
            .upsert_domain("one", None, crate::store::DomainKind::Virtual)
            .await
            .unwrap();
        let pool = short_pool(&t, &o);
        drop(pool.acquire().await.unwrap());
        // Once the lock is released, a new connection of the same pool is
        // refused.
        let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
            .bind(fence.key)
            .fetch_one(&mut lock)
            .await
            .unwrap();
        assert!(released);
        pool.close().await;
        let Err(err) = short_pool(&t, &o).acquire().await else {
            panic!("a pool connection went ahead after the lock was released");
        };
        assert!(matches!(err, sqlx::Error::PoolTimedOut), "{err}");
        lock.close().await.unwrap();
    }

    /// The lock connection carries its timeouts from the first statement.
    #[tokio::test]
    async fn lock_session_has_idle_and_statement_timeouts() {
        let Some(url) = pg_url() else {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
            return;
        };
        let t = PgTarget { url, schema: None };
        let mut lock = connect_one(&t, PgRole::Lock, &PgSessionOptions::default())
            .await
            .unwrap();
        let idle: String = sqlx::query_scalar("SHOW idle_session_timeout")
            .fetch_one(&mut lock)
            .await
            .unwrap();
        let statement: String = sqlx::query_scalar("SHOW statement_timeout")
            .fetch_one(&mut lock)
            .await
            .unwrap();
        assert_eq!((idle.as_str(), statement.as_str()), ("15s", "3s"));
        let keepalive: String = sqlx::query_scalar("SHOW tcp_keepalives_count")
            .fetch_one(&mut lock)
            .await
            .unwrap();
        // A Unix socket has no keepalives and shows 0.
        assert!(keepalive == "3" || keepalive == "0", "{keepalive}");
        lock.close().await.unwrap();
    }
}
