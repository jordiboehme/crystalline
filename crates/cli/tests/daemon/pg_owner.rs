//! The owner lock on Postgres, end to end with the real binary: a second
//! `serve` on an owned database waits as a standby, answers its probes and
//! its socket without ever opening the store, and stops cleanly. Plus the
//! Turso start, whose order the standby phase must not move.
//!
//! The Postgres tests skip with a note unless `CRYSTALLINE_TEST_POSTGRES_URL`
//! is set. Each runs on a scratch schema of its own ([`PgScratch`]).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::service::{Env, PgScratch, bin, free_port};

/// The owner timings divided by this in every child, so a standby polls
/// every 200 ms.
const TIMING_SCALE: &str = "10";

/// One `crystalline serve` child with an HTTP endpoint on a free port and its
/// stderr in a file. Stopped with SIGTERM when dropped.
pub(crate) struct Serve {
    child: Child,
    pub(crate) addr: String,
    log: PathBuf,
}

impl Serve {
    pub(crate) fn start(env: &Env, rust_log: Option<&str>) -> Serve {
        Serve::start_with(env, rust_log, &[])
    }

    /// [`Serve::start`] with `extra` variables for this child only.
    pub(crate) fn start_with(env: &Env, rust_log: Option<&str>, extra: &[(&str, &str)]) -> Serve {
        Serve::start_bound(env, "127.0.0.1", rust_log, extra)
    }

    /// [`Serve::start_with`] bound on `host`; requests still go to
    /// 127.0.0.1. A bind on `0.0.0.0` is one others can reach, where the
    /// first admin and the setup token are asked of the accounts.
    pub(crate) fn start_bound(
        env: &Env,
        host: &str,
        rust_log: Option<&str>,
        extra: &[(&str, &str)],
    ) -> Serve {
        let port = free_port();
        let bind = format!("{host}:{port}");
        let addr = format!("127.0.0.1:{port}");
        let log = env
            .state_dir()
            .with_file_name(format!("serve-{}.log", free_port()));
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        cmd.env("CRYSTALLINE_TEST_OWNER_TIMING_SCALE", TIMING_SCALE);
        if let Some(filter) = rust_log {
            cmd.env("RUST_LOG", filter);
        }
        for (key, value) in extra {
            cmd.env(key, value);
        }
        let child = cmd
            .args(["serve", "--http", &bind])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        Serve { child, addr, log }
    }

    pub(crate) fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    pub(crate) fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn get(&self, path: &str) -> Option<reqwest::blocking::Response> {
        reqwest::blocking::Client::new()
            .get(format!("http://{}{path}", self.addr))
            .timeout(Duration::from_secs(2))
            .send()
            .ok()
    }

    /// Wait until `/health` reports `role`, or panic with the log.
    pub(crate) fn wait_role(&mut self, role: &str) {
        let start = Instant::now();
        loop {
            if let Some(response) = self.get("/health")
                && let Ok(json) = response.json::<Value>()
                && json["role"] == role
            {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("serve exited with {status}:\n{}", self.log());
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "/health never said role {role}:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Wait until `/ready` answers 200, or panic with the log.
    pub(crate) fn wait_ready(&mut self) {
        let start = Instant::now();
        loop {
            if self
                .get("/ready")
                .is_some_and(|r| r.status().as_u16() == 200)
            {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("serve exited with {status}:\n{}", self.log());
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "/ready never answered 200:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Wait for the process to end and answer its exit code.
    pub(crate) fn wait_exit(&mut self, within: Duration) -> Option<i32> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            assert!(
                start.elapsed() < within,
                "serve did not exit within {within:?}:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Wait until the log holds `needle`, or panic with the log.
    fn wait_log(&mut self, needle: &str) {
        let start = Instant::now();
        while !self.log().contains(needle) {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("serve exited with {status}:\n{}", self.log());
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "the log never said {needle:?}:\n{}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub(crate) fn alive(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    pub(crate) fn terminate(&mut self) {
        unsafe {
            libc::kill(self.pid(), libc::SIGTERM);
        }
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.terminate();
            let start = Instant::now();
            while self.child.try_wait().ok().flatten().is_none()
                && start.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// An active instance on `pg` and a second `serve` that waits beside it,
/// each in a state folder of its own.
fn active_and_standby(pg: &PgScratch, tag: &str) -> (Env, Serve, Env, Serve) {
    let active_env = Env::new(&format!("{tag}a")).on_postgres(pg);
    let mut active = Serve::start(&active_env, Some("info,crystalline::start=debug"));
    active.wait_ready();
    let standby_env = Env::new(&format!("{tag}s")).on_postgres(pg);
    let mut standby = Serve::start(&standby_env, Some("info,crystalline::start=debug"));
    standby.wait_role("standby");
    (active_env, active, standby_env, standby)
}

/// A second `serve` on an owned database waits as a standby: `/health` is
/// 200 with `role` standby, `/ready` is 503, the API is 503 with
/// `Retry-After: 5`, a ctl command is refused as a standby's and `status`
/// shows the role. On Postgres the record is published before the store
/// opens, and the standby logs who owns the database.
#[test]
fn second_serve_waits_as_standby() {
    let Some(pg) = PgScratch::new("sbwait") else {
        return;
    };
    let (_active_env, active, standby_env, standby) = active_and_standby(&pg, "sbw");

    let log = active.log();
    let published = log.find("start: record published").expect(&log);
    let opened = log.find("start: store open").expect(&log);
    assert!(
        published < opened,
        "on Postgres the record comes first:\n{log}"
    );

    let health: Value = standby.get("/health").unwrap().json().unwrap();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["role"], "standby");
    assert_eq!(standby.get("/ready").unwrap().status().as_u16(), 503);
    let api = standby.get("/api/v1/domains").unwrap();
    assert_eq!(api.status().as_u16(), 503);
    assert_eq!(
        api.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("5")
    );
    let body: Value = api.json().unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("standby"),
        "{body}"
    );

    let (ok, out, err) = standby_env.run_full(&["config", "set", "service.read_only", "true"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("this instance is a standby"), "{err}");

    let (ok, out, err) = standby_env.run_full(&["status"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("Daemon: standby"), "{out}");
    let (ok, out, err) = standby_env.run_full(&["status", "--json"]);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(status["role"], "standby");
    assert_eq!(status["pid"].as_u64(), Some(standby.child.id() as u64));
    // A standby answers no `update_facts`, so `status` reads the update
    // facts from this binary and the cache file in its state folder.
    std::fs::create_dir_all(standby_env.state_dir()).unwrap();
    std::fs::write(
        standby_env.state_dir().join("update-check.json"),
        r#"{"v":1,"checked_at":"2026-11-18T08:00:00Z","outcome":{"kind":"release","version":"v99.0.0","published_at":"2026-01-01T00:00:00Z","first_seen":"2026-11-18T08:00:00Z"}}"#,
    )
    .unwrap();
    let mut cmd = Command::new(bin());
    standby_env.apply(&mut cmd);
    let out = cmd
        .env_remove("NO_UPDATE_NOTIFIER")
        .env_remove("CI")
        .env("CRYSTALLINE_UPDATE_CHECK", "true")
        .arg("status")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    assert!(out.contains("Daemon: standby"), "{out}");
    assert!(
        out.contains("\nUpdates: update check on; Crystalline 99.0.0 is available"),
        "{out}"
    );

    let log = standby.log();
    assert!(
        log.contains("standby: the database") && log.contains("; waiting"),
        "{log}"
    );
    assert!(!log.contains("start: store open"), "{log}");
    // The active one is untouched by all of it.
    assert_eq!(active.get("/ready").unwrap().status().as_u16(), 200);
}

/// `crystalline mcp` against a standby's state folder gets the standby's
/// sentence, as the initialize error and on stderr, and the standby is not
/// dislodged: its pid stays alive and it still answers as a standby.
#[test]
fn standby_is_never_dislodged() {
    let Some(pg) = PgScratch::new("sbmcp") else {
        return;
    };
    let (_active_env, _active, standby_env, mut standby) = active_and_standby(&pg, "sbm");

    let mut cmd = Command::new(bin());
    standby_env.apply(&mut cmd);
    let mut mcp = cmd
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let initialize = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "it", "version": "0" },
        },
    });
    let mut stdin = mcp.stdin.take().unwrap();
    writeln!(stdin, "{initialize}").unwrap();
    drop(stdin);
    let out = mcp.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stdout}{stderr}");
    let reply: Value = serde_json::from_str(stdout.lines().next().unwrap_or("null")).unwrap();
    assert_eq!(reply["id"], 1, "{stdout}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("this instance is a standby")),
        "{stdout}"
    );
    assert!(stderr.contains("this instance is a standby"), "{stderr}");

    assert!(
        standby.alive(),
        "the standby was dislodged:\n{}",
        standby.log()
    );
    assert_eq!(unsafe { libc::kill(standby.pid(), 0) }, 0);
    let health: Value = standby.get("/health").unwrap().json().unwrap();
    assert_eq!(health["role"], "standby");
    assert_eq!(standby_env.lock_pid(), Some(standby.child.id() as u64));
}

/// A standby on an empty schema creates nothing in it: it never opens the
/// store and never migrates. The lock is held by the test's own session.
#[cfg(feature = "postgres")]
#[test]
fn standby_creates_nothing_on_an_empty_schema() {
    let Some(pg) = PgScratch::new("sbempty") else {
        return;
    };
    let url = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    let holder = runtime.block_on(async {
        use sqlx::Connection;
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(key)
            .execute(&mut conn)
            .await
            .unwrap();
        conn
    });

    let env = Env::new("sbempty").on_postgres(&pg);
    let mut standby = Serve::start(&env, None);
    standby.wait_role("standby");
    // Several polls at the scaled period.
    std::thread::sleep(Duration::from_secs(1));
    assert!(standby.alive(), "{}", standby.log());
    let objects = pg.text(&format!(
        "SELECT count(*)::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = '{}'",
        pg.schema
    ));
    assert_eq!(objects, "0", "the standby created objects in its schema");
    let (ok, out, err) = env.run_full(&["status", "--json"]);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(status["role"], "standby");
    assert!(status["owner"].is_null(), "{status}");
    assert!(
        standby
            .log()
            .contains("owned by an instance that has not announced itself yet"),
        "{}",
        standby.log()
    );

    standby.terminate();
    assert_eq!(standby.wait_exit(Duration::from_secs(10)), Some(0));
    runtime.block_on(async {
        use sqlx::Connection;
        holder.close().await.unwrap();
    });
}

/// A standby stops with exit 0 on ctl `shutdown` and on SIGTERM, and
/// removes its record on the way: there is nothing to release.
#[test]
fn standby_shutdown_exits_zero() {
    let Some(pg) = PgScratch::new("sbstop") else {
        return;
    };
    let (_active_env, _active, standby_env, mut standby) = active_and_standby(&pg, "sbx");

    let (ok, out, err) = standby_env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
    assert_eq!(standby.wait_exit(Duration::from_secs(10)), Some(0));
    assert!(!standby_env.info_path().exists(), "the record stayed");

    let mut again = Serve::start(&standby_env, None);
    again.wait_role("standby");
    again.terminate();
    assert_eq!(again.wait_exit(Duration::from_secs(10)), Some(0));
    assert!(!standby_env.info_path().exists(), "the record stayed");
}

/// On Turso the order of the start is unchanged: the store opens first and
/// the record is published only after it.
#[test]
fn turso_order_unchanged() {
    let env = Env::new("tursoorder");
    let mut serve = Serve::start(&env, Some("info,crystalline::start=debug"));
    serve.wait_ready();
    let log = serve.log();
    let opened = log.find("start: store open").expect(&log);
    let published = log.find("start: record published").expect(&log);
    assert!(
        opened < published,
        "the record came before the store:\n{log}"
    );
    let health: Value = serve.get("/health").unwrap().json().unwrap();
    assert_eq!(health["role"], "active");
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
    let log = serve.log();
    assert!(
        !log.contains("releasing the database"),
        "Turso has no database to release:\n{log}"
    );
}

/// The variables that take the secret key away from a child: a blank value
/// counts as unset, whatever the shell running the tests has set.
#[cfg(feature = "postgres")]
pub(crate) const NO_SECRET_KEY: [(&str, &str); 2] = [
    ("CRYSTALLINE_SECRET_KEY", ""),
    ("CRYSTALLINE_SECRET_KEY_FILE", ""),
];

/// How many rows `pg`'s `instance_secret` holds.
#[cfg(feature = "postgres")]
pub(crate) fn secret_rows(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT count(*)::text FROM {}.instance_secret",
        pg.schema
    ))
}

/// A start of `env` that stops by itself: its exit code and its log.
#[cfg(feature = "postgres")]
pub(crate) fn failed_start(env: &Env, extra: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut serve = Serve::start_with(env, Some("info"), extra);
    let code = serve.wait_exit(Duration::from_secs(60));
    (code, serve.log())
}

/// A first start with the key, which migrates the schema; answers the id of
/// the key from its log.
#[cfg(feature = "postgres")]
fn migrated_by_a_start(env: &Env) -> String {
    let mut serve = Serve::start(env, Some("info"));
    serve.wait_ready();
    let log = serve.log();
    let marker = "secrets sealed in postgres with key ";
    let at = log.find(marker).expect(&log) + marker.len();
    let key_id: String = log[at..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
    key_id
}

/// On Postgres the secret key is required: a start without it stops before
/// it takes the lock, with a sentence naming the variable and how to make a
/// key, on an empty schema (nothing is created) and on one with sealed rows
/// (they are kept).
#[cfg(feature = "postgres")]
#[test]
fn missing_key_fails_start() {
    let Some(pg) = PgScratch::new("skmiss") else {
        return;
    };
    let env = Env::new("skmiss").on_postgres(&pg);
    let (code, log) = failed_start(&env, &NO_SECRET_KEY);
    assert_ne!(code, Some(0), "{log}");
    assert!(log.contains("CRYSTALLINE_SECRET_KEY"), "{log}");
    assert!(log.contains("openssl rand -base64 32"), "{log}");
    assert_eq!(schema_objects(&pg), "0", "nothing was created:\n{log}");

    migrated_by_a_start(&env);
    pg.run(&format!(
        "INSERT INTO {}.instance_secret VALUES ('github', '\\x01'::bytea, 'k', now())",
        pg.schema
    ))
    .unwrap();
    let (code, log) = failed_start(&env, &NO_SECRET_KEY);
    assert_ne!(code, Some(0), "{log}");
    assert!(log.contains("CRYSTALLINE_SECRET_KEY"), "{log}");
    assert!(
        owner_command(&pg).ends_with("|true"),
        "the keyless start never took the database"
    );
    assert_eq!(secret_rows(&pg), "1", "the row is kept");
}

/// Rows sealed with another key stop the start after the takeover, naming
/// both key ids, and every row is kept.
#[cfg(feature = "postgres")]
#[test]
fn key_id_mismatch_fails_start_and_keeps_rows() {
    let Some(pg) = PgScratch::new("skwrong") else {
        return;
    };
    let env = Env::new("skwrong").on_postgres(&pg);
    let mine = migrated_by_a_start(&env);
    assert_eq!(mine.len(), 16, "the key id is logged");
    pg.run(&format!(
        "INSERT INTO {s}.instance_secret VALUES ('github', '\\x01'::bytea, '0123456789abcdef', now()); \
         INSERT INTO {s}.instance_secret VALUES ('github-app', '\\x01'::bytea, '0123456789abcdef', now())",
        s = pg.schema
    ))
    .unwrap();
    let (code, log) = failed_start(&env, &[]);
    assert_ne!(code, Some(0), "{log}");
    assert!(log.contains("0123456789abcdef"), "{log}");
    assert!(log.contains(&mine), "{log}");
    assert_eq!(secret_rows(&pg), "2", "the rows are kept:\n{log}");
}

/// A credential the daemon reads right after its start comes from the
/// vault: an instance token sealed in the database is what the GitHub
/// settings report, and a leftover token file in the state folder is not
/// read.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn a_credential_read_after_start_comes_from_the_vault() {
    use crystalline_remote::sealed::{SealKey, seal};
    use crystalline_remote::{StoredToken, TokenIdentity, TokenStore};

    use crate::service::TEST_SECRET_KEY;

    let Some(pg) = PgScratch::new("skread") else {
        return;
    };
    let env = Env::new("skread").on_postgres(&pg);
    migrated_by_a_start(&env);
    let token = |user: &str| StoredToken {
        access_token: format!("{user}-secret"),
        host: "github.com".to_string(),
        user: user.to_string(),
        created_at: crystalline_core::time::now(),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    };
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&TEST_SECRET_KEY[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let key = SealKey::from_bytes(bytes.try_into().unwrap());
    let sealed = seal(
        &key,
        "github",
        &serde_json::to_vec(&token("vault-user")).unwrap(),
    );
    let hex: String = sealed.iter().map(|b| format!("{b:02x}")).collect();
    pg.run(&format!(
        "INSERT INTO {}.instance_secret VALUES ('github', decode('{hex}', 'hex'), '{}', now())",
        pg.schema,
        key.key_id()
    ))
    .unwrap();
    let origins = env.state_dir().join("origins");
    TokenStore::file_fallback_for(&TokenIdentity::Instance, &origins)
        .unwrap()
        .save(&token("file-user"))
        .unwrap();

    let mut serve = Serve::start_bound(
        &env,
        "0.0.0.0",
        Some("info"),
        &[
            ("CRYSTALLINE_GITHUB_ENABLED", "true"),
            ("CRYSTALLINE_ADMIN_NAME", "root"),
            ("CRYSTALLINE_ADMIN_PASSWORD", "root-password"),
        ],
    );
    serve.wait_ready();
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    let login = client
        .post(format!("http://{}/api/v1/auth/login", serve.addr))
        .json(&serde_json::json!({ "name": "root", "password": "root-password" }))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap();
    assert_eq!(login.status().as_u16(), 200);
    let cookies: Vec<String> = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_string))
        .collect();
    let github: Value = client
        .get(format!("http://{}/api/v1/settings/github", serve.addr))
        .header("cookie", cookies.join("; "))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(github["token_store"], "vault", "{github}");
    assert_eq!(github["user"], "vault-user", "{github}");
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
}

/// On Turso the secret key is ignored with one warning, even one that
/// would not parse, and the instance serves.
#[test]
fn turso_start_with_a_secret_key_warns_once_and_serves() {
    let env = Env::new("tursokey");
    let mut serve = Serve::start_with(
        &env,
        Some("info"),
        &[("CRYSTALLINE_SECRET_KEY", "not-a-key")],
    );
    serve.wait_ready();
    let log = serve.log();
    assert_eq!(
        log.matches("The key is ignored").count(),
        1,
        "one warning:\n{log}"
    );
    assert!(!log.contains("not-a-key"), "the value is never logged");
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
}

/// The log line a former standby writes when the start hold begins.
const HOLDING: &str = "test hook: holding the start";

/// A standby that took the database over and is still starting (held by
/// the test hook for 30 s) stops at once on SIGTERM, with exit 0, and gives
/// the database and its record back: the stop signals are one future for
/// the whole process, so the standby wait did not swallow them.
#[cfg(feature = "postgres")]
#[test]
fn a_former_standby_stops_at_once_during_its_start() {
    let Some(pg) = PgScratch::new("sbstart") else {
        return;
    };
    let active_env = Env::new("sbsa").on_postgres(&pg);
    let mut active = Serve::start(&active_env, None);
    active.wait_ready();
    let standby_env = Env::new("sbss").on_postgres(&pg);
    let mut standby = Serve::start_with(
        &standby_env,
        None,
        &[("CRYSTALLINE_TEST_START_HOLD_MS", "30000")],
    );
    standby.wait_role("standby");

    let (ok, out, err) = active_env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
    assert_eq!(active.wait_exit(Duration::from_secs(15)), Some(0));
    standby.wait_log(HOLDING);

    let asked = Instant::now();
    standby.terminate();
    assert_eq!(standby.wait_exit(Duration::from_secs(10)), Some(0));
    assert!(
        asked.elapsed() < Duration::from_secs(10),
        "the stop waited for the start"
    );
    let log = standby.log();
    assert!(log.contains("start: stopping (SIGTERM)"), "{log}");
    assert!(!standby_env.info_path().exists(), "the record stayed");
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    assert_eq!(
        pg.text(&format!("SELECT pg_try_advisory_lock({key})::text")),
        "true",
        "the database is still held"
    );
}

/// A connection made after a former standby took the lock and before the
/// daemon serves is parked and then served, never refused: an MCP session
/// gets its initialize result and `status` the daemon's own report.
#[test]
fn a_former_standby_serves_what_its_start_parked() {
    let Some(pg) = PgScratch::new("sbpark") else {
        return;
    };
    let active_env = Env::new("sbpa").on_postgres(&pg);
    let mut active = Serve::start(&active_env, None);
    active.wait_ready();
    let standby_env = Env::new("sbps").on_postgres(&pg);
    let mut standby = Serve::start_with(
        &standby_env,
        None,
        &[("CRYSTALLINE_TEST_START_HOLD_MS", "3000")],
    );
    standby.wait_role("standby");

    let (ok, out, err) = active_env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
    assert_eq!(active.wait_exit(Duration::from_secs(15)), Some(0));
    standby.wait_log(HOLDING);
    // Not ready while it starts: the full router is not in yet.
    assert_eq!(standby.get("/ready").unwrap().status().as_u16(), 503);

    // Both asked inside the hold.
    let status = {
        let mut cmd = Command::new(bin());
        standby_env.apply(&mut cmd);
        cmd.args(["status", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut cmd = Command::new(bin());
    standby_env.apply(&mut cmd);
    let mut mcp = cmd
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = mcp.stdin.take().unwrap();
    let initialize = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "it", "version": "0" },
        },
    });
    writeln!(stdin, "{initialize}").unwrap();
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(mcp.stdout.take().unwrap()),
        &mut first,
    )
    .unwrap();
    drop(stdin);
    let _ = mcp.wait();
    let reply: Value = serde_json::from_str(first.trim()).unwrap();
    assert_eq!(reply["id"], 1, "{first}");
    assert!(
        reply.get("result").is_some(),
        "the session was refused: {first}"
    );

    let out = status.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_ne!(report["role"], "standby", "{report}");
    assert_eq!(report["pid"].as_u64(), Some(standby.child.id() as u64));

    let health: Value = standby.get("/health").unwrap().json().unwrap();
    assert_eq!(health["role"], "active");
    standby.wait_ready();
}

/// This environment's instance id, as the daemon read or made it.
pub(crate) fn instance_id(env: &Env) -> String {
    std::fs::read_to_string(env.state_dir().join("instance-id"))
        .expect("the instance id")
        .trim()
        .to_string()
}

/// The owner row of `pg`'s schema as `instance_id|released`, where
/// `released` is `t` or `f`; empty when there is no row.
pub(crate) fn owner_row(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT instance_id || '|' || (released_at IS NOT NULL)::text FROM {}.instance_owner WHERE id = 1",
        pg.schema
    ))
}

/// Wait until `check` holds, or panic with `what` after `within`.
fn wait_until(within: Duration, what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < within, "{what} within {within:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A clean stop gives the database back (`released_at` and the unlock, as
/// the last shutdown step), so the standby takes over at its next poll and
/// skips the grace: it serves in under 3 s.
#[test]
fn graceful_stop_hands_over_without_grace() {
    let Some(pg) = PgScratch::new("hograce") else {
        return;
    };
    let (active_env, mut active, standby_env, mut standby) = active_and_standby(&pg, "hog");

    let (ok, out, err) = active_env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
    let asked = Instant::now();
    assert_eq!(active.wait_exit(Duration::from_secs(15)), Some(0));
    standby.wait_ready();
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "the handover took {:?}:\n{}",
        asked.elapsed(),
        standby.log()
    );
    let log = active.log();
    let settled = log
        .find("shutdown: saving a refreshed sign-in")
        .expect(&log);
    let released = log.find("shutdown: releasing the database").expect(&log);
    assert!(settled < released, "the release is the last step:\n{log}");
    let log = standby.log();
    assert!(log.contains("without a grace"), "{log}");
    assert_eq!(
        owner_row(&pg),
        format!("{}|false", instance_id(&standby_env))
    );
}

/// A killed owner releases nothing: the standby gets the lock once the
/// server ends the dead session, waits out the grace, and then serves.
#[test]
fn kill_9_hands_over_after_grace() {
    let Some(pg) = PgScratch::new("hokill") else {
        return;
    };
    let (_active_env, mut active, standby_env, mut standby) = active_and_standby(&pg, "hok");

    unsafe {
        libc::kill(active.pid(), libc::SIGKILL);
    }
    let _ = active.wait_exit(Duration::from_secs(10));
    standby.wait_ready();
    let log = standby.log();
    assert!(log.contains("after a grace of"), "{log}");
    assert_eq!(
        owner_row(&pg),
        format!("{}|false", instance_id(&standby_env))
    );
}

/// An owner whose lock session is ended from outside is fenced: it exits
/// with code 4, without the departure's steps, and the standby takes over.
#[test]
fn terminated_lock_session_exits_with_fenced_code() {
    let Some(pg) = PgScratch::new("hofence") else {
        return;
    };
    let (_active_env, mut active, standby_env, mut standby) = active_and_standby(&pg, "hof");

    assert_eq!(
        pg.text(&format!(
            "SELECT pg_terminate_backend(backend_pid::int)::text FROM {}.instance_owner",
            pg.schema
        )),
        "true"
    );
    assert_eq!(active.wait_exit(Duration::from_secs(15)), Some(4));
    let log = active.log();
    assert!(
        log.contains("lost ownership of the database")
            && log.contains("stopping so another instance can take over"),
        "{log}"
    );
    assert!(
        !log.contains("shutdown:"),
        "a fenced exit ran the departure:\n{log}"
    );
    standby.wait_ready();
    assert_eq!(
        owner_row(&pg),
        format!("{}|false", instance_id(&standby_env))
    );
}

/// How long a fenced owner waits before its exit in the paused test: long
/// enough for its watcher to index the dropped file, were it let through.
const FENCE_EXIT_HOLD_MS: &str = "5000";

/// Sends SIGCONT to a stopped child when dropped, so a failed assertion
/// never leaves it stopped.
struct Paused(i32);

impl Paused {
    fn new(pid: i32) -> Paused {
        unsafe {
            libc::kill(pid, libc::SIGSTOP);
        }
        Paused(pid)
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0, libc::SIGCONT);
        }
    }
}

/// A paused owner cannot write after a takeover: while it is stopped the
/// server ends its idle lock session, the standby takes the lock, ends
/// every session of the paused owner and serves. A file dropped into the
/// paused owner's domain meanwhile is never indexed: once it wakes it is
/// fenced and exits with code 4, and the engram rows it wrote before the
/// pause are what is left. The fenced exit is held for a few seconds (a
/// debug-only test hook), so the woken owner's watcher really tries to
/// write the file and only the connect fence of its pools keeps it out.
#[test]
fn paused_owner_cannot_write_after_takeover() {
    let Some(pg) = PgScratch::new("hopause") else {
        return;
    };
    let active_env = Env::new("hopa").on_postgres(&pg);
    // Defined by a variable of the paused owner alone, not in the
    // configuration document: the new owner must not index the late file
    // itself, so a row more can only come from the paused owner.
    let domain_dir = active_env.domain_folder("notes");
    let mut active = Serve::start_with(
        &active_env,
        None,
        &[
            ("CRYSTALLINE_TEST_FENCE_EXIT_HOLD_MS", FENCE_EXIT_HOLD_MS),
            ("CRYSTALLINE_DOMAIN_NOTES", domain_dir.to_str().unwrap()),
        ],
    );
    active.wait_ready();
    let engrams = || pg.text(&format!("SELECT count(*)::text FROM {}.engram", pg.schema));
    wait_until(Duration::from_secs(30), "the seed was indexed", || {
        engrams().parse::<u64>().unwrap_or(0) > 0
    });
    // Settled: the count does not move for a second.
    let mut before = engrams();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = engrams();
        if now == before {
            break;
        }
        before = now;
    }
    let tag: String = instance_id(&active_env).chars().take(8).collect();
    let standby_env = Env::new("hops").on_postgres(&pg);
    let mut standby = Serve::start(&standby_env, None);
    standby.wait_role("standby");

    let paused = Paused::new(active.pid());
    std::fs::write(
        domain_dir.join("late.md"),
        "---\ntype: engram\ntitle: Late\npermalink: late\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nwritten while the owner was paused\n",
    )
    .unwrap();
    standby.wait_ready();
    let left = pg.text(&format!(
        "SELECT count(*)::text FROM pg_stat_activity WHERE split_part(application_name, '/', 3) = '{tag}'"
    ));
    assert_eq!(
        left, "0",
        "a session of the paused owner outlived the takeover"
    );

    drop(paused);
    active.wait_log("test hook: holding the fenced exit");
    assert_eq!(active.wait_exit(Duration::from_secs(20)), Some(4));
    assert_eq!(
        engrams(),
        before,
        "the paused owner wrote after the takeover"
    );
    assert!(standby.alive(), "{}", standby.log());
}

/// A clean stop records `released_at` in the owner row and frees the lock.
#[test]
fn released_at_set_on_clean_stop() {
    let Some(pg) = PgScratch::new("horel") else {
        return;
    };
    let env = Env::new("horel").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let me = instance_id(&env);
    assert_eq!(owner_row(&pg), format!("{me}|false"));

    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
    assert_eq!(owner_row(&pg), format!("{me}|true"));
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    assert_eq!(
        pg.text(&format!("SELECT pg_try_advisory_lock({key})::text")),
        "true",
        "the database is still held"
    );
}

/// The owner of a Postgres database claims the host lock of each of its
/// domains with take-over: after a killed owner, the new one hosts the
/// domain at once, long before the dead one's heartbeat goes stale.
#[test]
fn new_owner_takes_host_locks_at_once_after_kill() {
    let Some(pg) = PgScratch::new("hohost") else {
        return;
    };
    let stale: &[(&str, &str)] = &[("CRYSTALLINE_STALE_SECS", "600")];
    let active_env = Env::new("hoha").on_postgres(&pg);
    active_env.setup_domain("notes");
    // The registration is the database's configuration document, so the
    // second instance serves the same domain without adding it.
    let standby_env = Env::new("hohs").on_postgres(&pg);
    let holder = || {
        pg.text(&format!(
            "SELECT l.holder_instance_id FROM {s}.domain_lock l JOIN {s}.domain d ON d.id = l.domain_id \
             WHERE d.name = 'notes'",
            s = pg.schema
        ))
    };

    let mut active = Serve::start_with(&active_env, None, stale);
    active.wait_ready();
    let first = instance_id(&active_env);
    wait_until(
        Duration::from_secs(30),
        "the first owner hosts notes",
        || holder() == first,
    );
    let mut standby = Serve::start_with(&standby_env, None, stale);
    standby.wait_role("standby");

    unsafe {
        libc::kill(active.pid(), libc::SIGKILL);
    }
    let _ = active.wait_exit(Duration::from_secs(10));
    standby.wait_ready();
    let second = instance_id(&standby_env);
    wait_until(Duration::from_secs(15), "the new owner hosts notes", || {
        holder() == second
    });
}

/// A login role made for one test, dropped with its grants afterwards.
#[cfg(feature = "postgres")]
struct LowRole<'a> {
    pg: &'a PgScratch,
    name: String,
}

#[cfg(feature = "postgres")]
impl Drop for LowRole<'_> {
    fn drop(&mut self) {
        let _ = self
            .pg
            .sql(&format!("DROP OWNED BY {0}; DROP ROLE {0}", self.name));
    }
}

/// A takeover that may not end its predecessor's sessions stops the start
/// with the sentence that names the grant to give. The predecessor is a
/// session of a superuser tagged as an owner-process connection; the new
/// instance runs as a plain role, which may not end it.
#[cfg(feature = "postgres")]
#[test]
fn a_refused_takeover_names_the_grant() {
    let Some(pg) = PgScratch::new("hogrant") else {
        return;
    };
    let role = LowRole {
        pg: &pg,
        name: format!("cq_low_{}", std::process::id()),
    };
    assert!(pg.sql(&format!(
        "CREATE ROLE {0} LOGIN PASSWORD 'low'; GRANT ALL ON SCHEMA {1} TO {0}",
        role.name, pg.schema
    )));
    let url = pg.url.clone();
    let (scheme, rest) = url.split_once("://").unwrap();
    let host = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let low_url = format!("{scheme}://{}:low@{host}", role.name);

    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    let name = crystalline_index::postgres::application_name(
        key,
        "deadbeef",
        crystalline_index::postgres::PgRole::Pool,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (predecessor, seen) = runtime.block_on(async {
        use sqlx::Connection;
        let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let predecessor = sqlx::PgConnection::connect_with(&options.application_name(&name))
            .await
            .unwrap();
        // The plain role must see the predecessor's name, or the takeover
        // would match nothing and never be refused.
        let mut low = sqlx::PgConnection::connect(&low_url).await.unwrap();
        let seen: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name = $1")
                .bind(&name)
                .fetch_one(&mut low)
                .await
                .unwrap();
        let _ = low.close().await;
        (predecessor, seen)
    });
    assert_eq!(
        seen, 1,
        "a plain role does not see other roles' session names"
    );

    let env = Env::new("hogrant").on_postgres(&pg);
    let mut serve = Serve::start_with(&env, None, &[("CRYSTALLINE_DATABASE_URL", &low_url)]);
    let code = serve.wait_exit(Duration::from_secs(30));
    assert_ne!(code, Some(0), "{}", serve.log());
    let log = serve.log();
    assert!(
        log.contains("may not end the sessions of the instance before it")
            && log.contains("grant its database role pg_signal_backend")
            && !log.contains("invalid input"),
        "{log}"
    );
    runtime.block_on(async {
        use sqlx::Connection;
        let _ = predecessor.close().await;
    });
}

/// On Postgres the accounts live in the database only: when they cannot be
/// opened (here their tables are ahead of this binary), the start fails
/// with a sentence that says so, and the HTTP surface never falls back to
/// an accounts file on this machine.
#[test]
fn accounts_that_do_not_open_stop_the_start_on_postgres() {
    let Some(pg) = PgScratch::new("hoauth") else {
        return;
    };
    assert!(pg.sql(&format!(
        "CREATE TABLE {0}.auth_schema_migration (version BIGINT PRIMARY KEY, applied_at TEXT NOT NULL); \
         INSERT INTO {0}.auth_schema_migration VALUES (999, '2026-11-16T00:00:00Z')",
        pg.schema
    )));
    let env = Env::new("hoauth").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    let code = serve.wait_exit(Duration::from_secs(30));
    let log = serve.log();
    assert_ne!(code, Some(0), "{log}");
    assert!(
        log.contains("could not open the accounts in the database")
            && log.contains("does not start without them"),
        "{log}"
    );
    assert!(
        !env.state_dir().join("web-auth.db").exists(),
        "an accounts file was made"
    );
}

/// The `command` and `released` columns of `pg`'s owner row as
/// `command|released`, `command` empty when it is NULL; empty when there is
/// no row.
fn owner_command(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT coalesce(command, '') || '|' || (released_at IS NOT NULL)::text FROM {}.instance_owner WHERE id = 1",
        pg.schema
    ))
}

/// The label in `pg`'s owner row: the name the sentences give the owner.
fn owner_label(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT label FROM {}.instance_owner WHERE id = 1",
        pg.schema
    ))
}

/// How many objects `pg`'s schema holds.
#[cfg(feature = "postgres")]
fn schema_objects(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT count(*)::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = '{}'",
        pg.schema
    ))
}

/// Run `crystalline <args>` in `env` as an MCP client that sends one
/// `initialize` and closes stdin: the exit code, stdout and stderr. Gives up
/// after 60 s, so a bridge that loops fails the test instead of hanging it.
fn mcp_once(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    mcp_once_with(env, args, &[])
}

/// [`mcp_once`] with `extra` variables on top of `env`'s own.
fn mcp_once_with(
    env: &Env,
    args: &[&str],
    extra: &[(&str, &str)],
) -> (Option<i32>, String, String) {
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    for (key, value) in extra {
        cmd.env(key, value);
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let initialize = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "it", "version": "0" },
        },
    });
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{initialize}").unwrap();
    drop(stdin);
    let pid = child.id() as i32;
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let out = child.wait_with_output().unwrap();
        let _ = done_tx.send(());
        out
    });
    if done_rx.recv_timeout(Duration::from_secs(60)).is_err() {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let out = waiter.join().unwrap();
        panic!(
            "`crystalline {}` did not end within 60s:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = waiter.join().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The error message of the reply to the `initialize` [`mcp_once`] sent.
fn initialize_error(stdout: &str) -> String {
    let reply: Value = serde_json::from_str(stdout.lines().next().unwrap_or("null")).unwrap();
    assert_eq!(reply["id"], 1, "{stdout}");
    reply["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("no initialize error: {stdout}"))
        .to_string()
}

/// Without a daemon, a command that would open its own engine on a
/// database another instance owns is refused before anything opens, on
/// both openers (the CLI's index route and the engine of a data verb): the
/// sentence names the active instance and how to run the command there.
/// `config set` of a setting needs the owner too; one of a `database.*` key
/// does not.
#[test]
fn engine_command_without_daemon_refused_when_owned() {
    let Some(pg) = PgScratch::new("clown") else {
        return;
    };
    let owner_env = Env::new("clowa").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let label = owner_label(&pg);

    let env = Env::new("clowc").on_postgres(&pg);
    for args in [&["reindex"][..], &["search", "anything"]] {
        let (ok, out, err) = env.run_full(args);
        assert!(!ok, "{out}{err}");
        assert!(
            err.contains(&format!(
                "is owned by the active instance {label} (pid {},",
                owner.pid()
            )),
            "{err}"
        );
        assert!(
            err.contains(&format!(
                "for example `kubectl exec <pod> -- crystalline {}`",
                args.join(" ")
            )),
            "{err}"
        );
        assert!(!err.contains("could not reach"), "{err}");
    }
    // A setting lives in the configuration document the owner writes, so a
    // change without it is refused like any engine command (spec section C).
    let (ok, out, err) = env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(!ok, "{out}{err}");
    assert!(
        err.contains(&format!(
            "is owned by the active instance {label} (pid {},",
            owner.pid()
        )),
        "{err}"
    );
    // A `database.*` key stays config.yaml's and needs no owner: it is how
    // a machine is pointed away from a database another instance owns.
    let (ok, out, err) = env.run_full(&["config", "set", "database.schema", "elsewhere"]);
    assert!(ok, "{out}{err}");
    let yaml = std::fs::read_to_string(env.config_path()).unwrap();
    assert!(yaml.contains("elsewhere"), "{yaml}");

    assert_eq!(owner_command(&pg), "|false", "the owner row changed");
    assert_eq!(owner.get("/ready").unwrap().status().as_u16(), 200);
}

/// A refused command opens no engine: on an empty schema whose lock the
/// test's own session holds, it creates nothing, says the holder has not
/// announced itself, and a read-only report (`status`) opens nothing
/// either.
#[cfg(feature = "postgres")]
#[test]
fn command_without_daemon_opens_nothing_when_owned() {
    let Some(pg) = PgScratch::new("clnone") else {
        return;
    };
    let url = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    let holder = runtime.block_on(async {
        use sqlx::Connection;
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(key)
            .execute(&mut conn)
            .await
            .unwrap();
        conn
    });

    let env = Env::new("clnone").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["reindex"]);
    assert!(!ok, "{out}{err}");
    assert!(
        err.contains("is owned by an instance that has not announced itself yet"),
        "{err}"
    );
    let (_, out, err) = env.run_full(&["status"]);
    assert!(
        format!("{out}{err}").contains("has not announced itself yet"),
        "{out}{err}"
    );
    assert_eq!(
        schema_objects(&pg),
        "0",
        "a refused command created objects"
    );

    runtime.block_on(async {
        use sqlx::Connection;
        holder.close().await.unwrap();
    });
}

/// Without a daemon on a free database, a command claims it for its run:
/// the owner row names the command while it runs and records the release
/// when it ends, and the lock is free again. A read-only report never
/// claims it.
#[cfg(feature = "postgres")]
#[test]
fn reindex_without_daemon_runs_and_releases_when_free() {
    let Some(pg) = PgScratch::new("clfree") else {
        return;
    };
    let env = Env::new("clfree").on_postgres(&pg);
    // The first command on the empty schema: its store open creates the
    // owner table after the claim, and the claim is recorded all the same.
    env.setup_domain("eng");
    assert_eq!(owner_command(&pg), "crystalline domain add|true");
    let (ok, out, err) = env.run_full(&["reindex"]);
    assert!(ok, "{out}{err}");
    assert_eq!(owner_command(&pg), "crystalline reindex|true");
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    assert_eq!(
        pg.text(&format!(
            "SELECT (count(*) = 0)::text FROM pg_locks WHERE locktype = 'advisory' \
             AND objid::bigint = ({key} & 4294967295)"
        )),
        "true",
        "the database is still held"
    );

    let (_, out, err) = env.run_full(&["status"]);
    assert!(
        format!("{out}{err}").contains("has no active instance"),
        "{out}{err}"
    );
    assert_eq!(
        owner_command(&pg),
        "crystalline reindex|true",
        "status claimed the database"
    );
}

/// A read-only report on Postgres never opens the store, free or owned
/// (only an owner migrates): on an empty schema `status` and `doctor`
/// create nothing and say there is no active instance, and a look that
/// fails counts as unavailable, never as free.
#[cfg(feature = "postgres")]
#[test]
fn reports_never_open_the_store_on_postgres() {
    let Some(pg) = PgScratch::new("clrep") else {
        return;
    };
    let env = Env::new("clrep").on_postgres(&pg);
    let (_, out, err) = env.run_full(&["status"]);
    assert!(
        format!("{out}{err}").contains("has no active instance"),
        "{out}{err}"
    );
    let (_, out, err) = env.run_full(&["doctor"]);
    assert!(!format!("{out}{err}").contains("is owned by"), "{out}{err}");
    assert_eq!(schema_objects(&pg), "0", "a report opened the store");

    // Nothing listens on port 1: the look fails.
    let unreachable = pg.url.replacen(
        &pg.url
            .split('@')
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap()
            .to_string(),
        "127.0.0.1:1",
        1,
    );
    let mut status = Command::new(bin());
    env.apply(&mut status);
    let out = status
        .env("CRYSTALLINE_DATABASE_URL", &unreachable)
        .arg("status")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("could not read who owns the database"),
        "{text}"
    );
}

/// A command of this machine that holds the database (here
/// `hold-lock --standalone`, the shape of a long `crystalline reindex`) is
/// waited for by `crystalline mcp`, as a one-shot command that holds the
/// state folder is: no sentence about another machine, and the session
/// starts once the command has finished.
#[test]
fn mcp_waits_for_a_command_of_this_machine() {
    mcp_waits_for_a_local_command("cllocal", false);
}

/// The same wait for a command of this machine that holds the database
/// but not the state folder (`users add`, `connect github` with the key):
/// the state folder is free, so only the owner row says who holds the
/// database, and the bridge waits for it instead of spawning a daemon that
/// would find the database held and leave.
#[test]
fn mcp_waits_for_a_command_that_holds_only_the_database() {
    mcp_waits_for_a_local_command("cldbonly", true);
}

/// The body of the two waits above: `database_only` keeps the holding
/// command off the state folder.
fn mcp_waits_for_a_local_command(name: &str, database_only: bool) {
    let Some(pg) = PgScratch::new(name) else {
        return;
    };
    let env = Env::new(name).on_postgres(&pg);
    env.setup_domain("eng");
    let mut hold = Command::new(bin());
    env.apply(&mut hold);
    hold.args([
        "hold-lock",
        "--secs",
        "4",
        "--standalone",
        "crystalline reindex",
    ]);
    if database_only {
        hold.arg("--database-only");
    }
    let started = std::time::Instant::now();
    let mut hold = hold
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(hold.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    assert_eq!(line.trim(), "holding");

    // Stdin stays open until the answer is in: a relaying bridge ends when
    // its client leaves.
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let log = env.state_dir().with_file_name("bridge.log");
    let mut bridge = cmd
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "it", "version": "0" },
            },
        })
    )
    .unwrap();
    let mut reply = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(bridge.stdout.take().unwrap()),
        &mut reply,
    )
    .unwrap();
    drop(stdin);
    let _ = bridge.wait();
    let err = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(!err.contains("does not serve it"), "{err}");
    let reply: Value = serde_json::from_str(reply.trim()).unwrap_or(Value::Null);
    assert!(reply["result"].is_object(), "{reply} {err}");
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "the session started before the command let go"
    );
    assert!(hold.wait().unwrap().success());
    let (ok, out, err) = env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
}

/// A bridge whose daemon of this machine restarts mid-session on Postgres
/// keeps the session: it reconnects to the next daemon, which takes the
/// database over, and answers the next request.
#[test]
fn bridge_survives_a_local_daemon_restart_on_postgres() {
    let Some(pg) = PgScratch::new("clrestart") else {
        return;
    };
    let env = Env::new("clrestart").on_postgres(&pg);
    env.setup_domain("eng");
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let mut bridge = cmd
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let (lines_tx, lines) = std::sync::mpsc::channel::<String>();
    let stdout = bridge.stdout.take().unwrap();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if lines_tx.send(line).is_err() {
                return;
            }
        }
    });
    let answer_to = |id: u64| -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no answer to request {id}"));
            let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if msg["id"] == id {
                return msg;
            }
        }
    };
    let send = |stdin: &mut std::process::ChildStdin, msg: Value| {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    };
    send(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "it", "version": "0" },
            },
        }),
    );
    assert!(answer_to(1)["result"].is_object());
    send(
        &mut stdin,
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    let first = env.lock_pid().expect("the bridge started a daemon");

    let (ok, out, err) = env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
    wait_until(Duration::from_secs(30), "a new daemon serves", || {
        env.lock_pid().is_some_and(|pid| pid != first)
    });
    send(
        &mut stdin,
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    let listed = answer_to(2);
    assert!(listed["result"]["tools"].is_array(), "{listed}");
    assert_ne!(owner_row(&pg), "", "the new daemon owns the database");

    drop(stdin);
    let _ = bridge.wait();
    let (ok, out, err) = env.run_full(&["ctl", "shutdown"]);
    assert!(ok, "{out}{err}");
}

/// A command holding the database (`hold-lock --standalone`, the shape of
/// a long `crystalline reindex`) makes a `serve` wait as a standby that
/// names the command, and the standby serves once the command has given
/// the database back.
#[test]
fn serve_waits_while_a_command_holds_then_serves() {
    let Some(pg) = PgScratch::new("clhold") else {
        return;
    };
    let cmd_env = Env::new("clholdc").on_postgres(&pg);
    // The owner table, so the command's claim is announced.
    cmd_env.setup_domain("eng");

    let mut hold = Command::new(bin());
    cmd_env.apply(&mut hold);
    let mut hold = hold
        .args([
            "hold-lock",
            "--secs",
            "6",
            "--standalone",
            "crystalline reindex",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(hold.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    assert_eq!(line.trim(), "holding");
    assert_eq!(owner_command(&pg), "crystalline reindex|false");

    let env = Env::new("clholds").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_role("standby");
    let (ok, out, err) = env.run_full(&["status", "--json"]);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        status["owner"]["command"], "crystalline reindex",
        "{status}"
    );

    assert!(hold.wait().unwrap().success());
    serve.wait_ready();
    assert_eq!(owner_row(&pg), format!("{}|false", instance_id(&env)));
    assert!(
        serve
            .log()
            .contains("without a grace (the instance before released it)"),
        "{}",
        serve.log()
    );
}

/// On an owned database an autostart never waits as a standby: `crystalline
/// mcp` spawns no daemon and gives the owner's sentence as the initialize
/// error and on its stderr; and a daemon started as an autostart anyway
/// (the race) exits at once with the lock-held code, even with a bounded
/// life.
#[test]
fn autostart_refused_with_owner_sentence_on_client_stderr() {
    let Some(pg) = PgScratch::new("clauto") else {
        return;
    };
    let owner_env = Env::new("clautoa").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let label = owner_label(&pg);
    let sentence = format!("is owned by {label} (since ");

    let env = Env::new("clautoc").on_postgres(&pg);
    let (code, out, err) = mcp_once(&env, &["mcp"]);
    assert_ne!(code, Some(0), "{out}{err}");
    let message = initialize_error(&out);
    assert!(
        message.contains(&sentence) && message.contains("this machine does not serve it"),
        "{message}"
    );
    assert!(
        err.contains(&sentence) && err.contains("this machine does not serve it"),
        "{err}"
    );
    assert!(
        !env.state_dir().join("daemon.log").exists(),
        "a daemon was spawned"
    );

    let log = env.state_dir().with_file_name("autostarted.log");
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let mut serve = Command::new(bin());
    env.apply(&mut serve);
    let mut serve = Serve {
        child: serve
            .args(["serve", "--autostarted", "--exit-when-idle"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
        addr: String::new(),
        log,
    };
    assert_eq!(serve.wait_exit(Duration::from_secs(20)), Some(3));
    let err = serve.log();
    assert!(
        err.contains(&sentence) && err.contains("this machine does not serve it"),
        "{err}"
    );
    assert!(!env.info_path().exists(), "the record stayed");
    assert_eq!(owner.get("/ready").unwrap().status().as_u16(), 200);
}

/// `crystalline mcp --embedded` on an owned database is refused with the
/// owner's sentence as the initialize error, not a status server.
#[test]
fn embedded_refused_when_owned() {
    let Some(pg) = PgScratch::new("clemb") else {
        return;
    };
    let owner_env = Env::new("clemba").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let label = owner_label(&pg);

    let env = Env::new("clembc").on_postgres(&pg);
    let (code, out, err) = mcp_once(&env, &["mcp", "--embedded"]);
    assert_ne!(code, Some(0), "{out}{err}");
    let message = initialize_error(&out);
    assert!(
        message.contains(&format!("is owned by {label} (since "))
            && message.contains("this machine does not serve it"),
        "{message}"
    );
    assert!(err.contains("this machine does not serve it"), "{err}");
    assert_eq!(owner_command(&pg), "|false", "the owner row changed");
}

/// A one-shot command next to a local standby reaches it over the socket
/// and gets its refusal, which names the active instance; it opens no
/// engine of its own and the standby keeps waiting.
#[test]
fn local_standby_refuses_one_shot_command() {
    let Some(pg) = PgScratch::new("clsb") else {
        return;
    };
    let (_active_env, _active, standby_env, mut standby) = active_and_standby(&pg, "clsb");
    let label = owner_label(&pg);
    for args in [&["reindex"][..], &["search", "anything"]] {
        let (ok, out, err) = standby_env.run_full(args);
        assert!(!ok, "{out}{err}");
        assert!(
            err.contains(&format!(
                "this instance is a standby; the active instance is {label}"
            )),
            "{err}"
        );
    }
    assert!(standby.alive(), "{}", standby.log());
    let health: Value = standby.get("/health").unwrap().json().unwrap();
    assert_eq!(health["role"], "standby");
    assert!(!standby.log().contains("start: store open"));
    assert_eq!(owner_command(&pg), "|false", "the owner row changed");
}

/// On Turso the standalone paths are as before: a command without a daemon
/// opens the index file, and `crystalline mcp --embedded` serves.
#[test]
fn standalone_paths_unchanged_on_turso() {
    let env = Env::new("clturso");
    env.setup_domain("eng");
    for args in [&["reindex"][..], &["search", "seed"], &["status"]] {
        let (ok, out, err) = env.run_full(args);
        assert!(ok, "{} failed: {out}{err}", args.join(" "));
        assert!(!err.contains("is owned by"), "{err}");
    }
    let (code, out, err) = mcp_once(&env, &["mcp", "--embedded"]);
    assert_eq!(code, Some(0), "{out}{err}");
    let reply: Value = serde_json::from_str(out.lines().next().unwrap_or("null")).unwrap();
    assert!(reply["result"].is_object(), "{out}");
}

/// Run `crystalline <args>` in `env` with `stdin` fed to it: success,
/// stdout and stderr.
pub(crate) fn run_with_stdin(env: &Env, args: &[&str], stdin: &str) -> (bool, String, String) {
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `crystalline users add <name> --password-stdin` in `env`, which must
/// succeed.
fn users_add(env: &Env, name: &str, password: &str) {
    let (ok, out, err) = run_with_stdin(
        env,
        &["users", "add", name, "--password-stdin"],
        &format!("{password}\n"),
    );
    assert!(ok, "users add {name} failed: {out}{err}");
}

/// The status a sign-in of `name` with `password` at `serve` gets.
pub(crate) fn sign_in(serve: &Serve, name: &str, password: &str) -> u16 {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{}/api/v1/auth/login", serve.addr))
        .json(&serde_json::json!({ "name": name, "password": password }))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap()
        .status()
        .as_u16()
}

/// The version `pg`'s accounts ledger records, `0` without a row.
fn auth_ledger(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT coalesce(max(version), 0)::text FROM {}.auth_schema_migration",
        pg.schema
    ))
}

/// On Postgres the daemon keeps every account in the database: a reachable
/// `serve` with an admin from the environment creates that admin there,
/// draws no setup token, signs it in, and no `web-auth.db` appears in its
/// state directory.
#[test]
fn daemon_on_postgres_creates_no_web_auth_db() {
    let Some(pg) = PgScratch::new("acnofile") else {
        return;
    };
    let env = Env::new("acnofile").on_postgres(&pg);
    let mut serve = Serve::start_bound(
        &env,
        "0.0.0.0",
        None,
        &[
            ("CRYSTALLINE_ADMIN_NAME", "root"),
            ("CRYSTALLINE_ADMIN_PASSWORD", "root-password"),
        ],
    );
    serve.wait_ready();
    let log = serve.log();
    assert!(
        log.contains("created the first admin 'root' from the environment"),
        "{log}"
    );
    assert!(!log.contains("setup token"), "{log}");
    assert_eq!(
        pg.text(&format!("SELECT count(*)::text FROM {}.users", pg.schema)),
        "1"
    );
    assert_eq!(sign_in(&serve, "root", "root-password"), 200);
    assert!(
        !env.state_dir().join("web-auth.db").exists(),
        "an accounts file was made"
    );
}

/// `users add` while a daemon owns the database writes the account into
/// the database beside it, as a second writer in the role `cli` with no
/// claim: the daemon signs the new account in at once, and the owner row
/// is the daemon's still.
#[test]
fn users_add_while_daemon_owns_is_seen_at_once() {
    let Some(pg) = PgScratch::new("acowned") else {
        return;
    };
    let env = Env::new("acowned").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    assert_eq!(sign_in(&serve, "ada", "ada-password"), 401);
    users_add(&env, "ada", "ada-password");
    assert_eq!(sign_in(&serve, "ada", "ada-password"), 200);
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(ok && out.contains("ada"), "{out}{err}");
    assert_eq!(owner_command(&pg), "|false", "the owner row changed");
    assert!(
        !env.state_dir().join("web-auth.db").exists(),
        "an accounts file was made"
    );
}

/// `agents` writes the accounts database beside a daemon that owns it, like
/// `users`: the agent lands in the database, no accounts file appears, the
/// owner row stays the daemon's, and its token reaches only the automation
/// verbs at the ctl route. `auth.mcp` is on, which agents need.
#[test]
fn agents_add_while_daemon_owns_lands_in_the_database() {
    let Some(pg) = PgScratch::new("agowned") else {
        return;
    };
    let env = Env::new("agowned")
        .on_postgres(&pg)
        .with_var("CRYSTALLINE_AUTH_MCP", "true");
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["--json", "agents", "add", "nightly"]);
    assert!(ok, "{out}{err}");
    let added: serde_json::Value = serde_json::from_str(&out).unwrap();
    let token = added["token"]["token"].as_str().unwrap().to_string();
    assert_eq!(
        pg.text(&format!(
            "SELECT count(*)::text FROM {}.agents WHERE name = 'nightly'",
            pg.schema
        )),
        "1"
    );
    let (ok, out, err) = env.run_full(&["agents", "list"]);
    assert!(ok && out.contains("nightly"), "{out}{err}");
    assert_eq!(owner_command(&pg), "|false", "the owner row changed");
    assert!(
        !env.state_dir().join("web-auth.db").exists(),
        "an accounts file was made"
    );
    let reply = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{}/api/v1/ctl", serve.addr))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "v": 1, "cmd": "status" }))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap();
    assert_eq!(reply.status().as_u16(), 200);
    let reply: serde_json::Value = reply.json().unwrap();
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"]
            .as_str()
            .is_some_and(|e| e.contains("only the automation verbs")),
        "{reply}"
    );
}

/// Only an owner migrates the accounts tables: with the ledger stamped
/// below this binary while a daemon owns the database, `users` is refused
/// naming both versions and where to run it, and migrates nothing.
#[test]
fn users_refused_when_auth_ledger_behind_and_owned() {
    let Some(pg) = PgScratch::new("acbehind") else {
        return;
    };
    let env = Env::new("acbehind").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    assert_eq!(auth_ledger(&pg), "2");
    assert!(pg.sql(&format!("DELETE FROM {}.auth_schema_migration", pg.schema)));
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(!ok, "{out}{err}");
    assert!(
        err.contains("the accounts tables are at version 0, this binary needs 2, and only the instance that owns the database migrates them: the database")
            && err.contains("is owned by the active instance")
            && err.matches("Run the command there").count() == 1,
        "{err}"
    );
    assert_eq!(
        auth_ledger(&pg),
        "0",
        "a command that does not own migrated"
    );
    assert_eq!(owner_command(&pg), "|false", "the owner row changed");
}

/// With the ledger behind and nobody owning the database, `users` takes it
/// for the command, migrates the accounts tables and gives it back.
#[cfg(feature = "postgres")]
#[test]
fn users_migrates_when_auth_ledger_behind_and_free() {
    let Some(pg) = PgScratch::new("acfree") else {
        return;
    };
    let env = Env::new("acfree").on_postgres(&pg);
    users_add(&env, "ada", "ada-password");
    assert_eq!(auth_ledger(&pg), "2");
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    assert_eq!(
        pg.text(&format!(
            "SELECT (count(*) = 0)::text FROM pg_locks WHERE locktype = 'advisory' \
             AND objid::bigint = ({key} & 4294967295)"
        )),
        "true",
        "the database is still held"
    );
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(ok && out.contains("ada"), "{out}{err}");

    // On a schema that has the index tables, the claim is recorded in the
    // owner row and given back with `released_at`.
    env.setup_domain("eng");
    assert!(pg.sql(&format!("DELETE FROM {}.auth_schema_migration", pg.schema)));
    users_add(&env, "bob", "bob-password");
    assert_eq!(auth_ledger(&pg), "2");
    assert_eq!(owner_command(&pg), "crystalline users add|true");
}

/// Accounts tables a newer Crystalline wrote are refused as too new, and
/// nothing is changed.
#[test]
fn users_refused_when_auth_ledger_too_new() {
    let Some(pg) = PgScratch::new("acnew") else {
        return;
    };
    assert!(pg.sql(&format!(
        "CREATE TABLE {0}.auth_schema_migration (version BIGINT PRIMARY KEY, applied_at TEXT NOT NULL); \
         INSERT INTO {0}.auth_schema_migration VALUES (999, '2026-11-16T00:00:00Z')",
        pg.schema
    )));
    let env = Env::new("acnew").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(!ok, "{out}{err}");
    assert!(
        err.contains("the accounts tables are at version 999, newer than this binary knows"),
        "{err}"
    );
    assert_eq!(auth_ledger(&pg), "999");
}

/// An accounts file left in the state directory of an instance on Postgres
/// is never read: its account does not sign in and is not listed, the file
/// is left as it was, and the daemon says once that it is not read.
#[test]
fn leftover_web_auth_db_not_read_and_warned() {
    let Some(pg) = PgScratch::new("acleft") else {
        return;
    };
    let turso = Env::new("acleft");
    users_add(&turso, "old", "old-password");
    let leftover = turso.state_dir().join("web-auth.db");
    let before = std::fs::read(&leftover).expect("the accounts file");
    let env = turso.on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let log = serve.log();
    let warning = format!("the accounts file {} is not read", leftover.display());
    assert_eq!(log.matches(&warning).count(), 1, "{log}");
    assert_eq!(sign_in(&serve, "old", "old-password"), 401);
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(ok && !out.contains("old"), "{out}{err}");
    drop(serve);
    assert_eq!(
        std::fs::read(&leftover).unwrap(),
        before,
        "the file changed"
    );
}

/// `domain visibility` and `domain members` write the accounts of the
/// configuration their `--config` names: with the default configuration on
/// Turso and the override on Postgres, the acl lands in the database and no
/// accounts file is made beside the default one.
#[test]
fn domain_visibility_writes_the_accounts_of_its_config_override() {
    let Some(pg) = PgScratch::new("acmem") else {
        return;
    };
    users_add(&Env::new("acmemp").on_postgres(&pg), "ada", "ada-password");
    let env = Env::new("acmem");
    let kb = env.state_dir().with_file_name("kb-eng");
    std::fs::create_dir_all(&kb).unwrap();
    let config = env.state_dir().with_file_name("pg.yaml");
    std::fs::write(
        &config,
        format!(
            "database:\n  backend: postgres\n  url: {}\n  schema: {}\ndomains:\n  eng:\n    path: {}\n",
            pg.url,
            pg.schema,
            kb.display()
        ),
    )
    .unwrap();
    let config = config.to_str().unwrap();
    let (ok, out, err) = env.run_full(&[
        "domain",
        "visibility",
        "eng",
        "private",
        "--owner",
        "ada",
        "--config",
        config,
    ]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        pg.text(&format!(
            "SELECT owner FROM {}.domain_acl WHERE domain = 'eng'",
            pg.schema
        )),
        "ada"
    );
    let (ok, out, err) = env.run_full(&["domain", "members", "eng", "list", "--config", config]);
    assert!(ok && out.contains("ada"), "{out}{err}");
    assert!(
        !env.state_dir().join("web-auth.db").exists(),
        "the default configuration's accounts file was written"
    );
}

/// A Postgres daemon starts the GitHub App load before it opens the
/// accounts, as every other stack that installs domain access does, so the
/// first request on a governed domain does not find the App unsettled.
#[test]
fn daemon_on_postgres_preloads_the_github_app() {
    let Some(pg) = PgScratch::new("acapp") else {
        return;
    };
    let env = Env::new("acapp").on_postgres(&pg);
    let mut serve = Serve::start(&env, Some("info,crystalline::start=debug"));
    serve.wait_ready();
    let log = serve.log();
    let preload = log.find("start: github app preload").expect(&log);
    let opened = log.find("start: store open").expect(&log);
    assert!(opened < preload, "{log}");
}

/// A rebuild never takes the accounts with it: on Turso `reindex --wipe`
/// keeps the accounts file, and on Postgres, where the wipe is refused, the
/// accounts tables stay through the refusal and through `reindex --full`.
#[test]
fn reindex_wipe_keeps_accounts() {
    let turso = Env::new("acwipet");
    turso.setup_domain("eng");
    users_add(&turso, "ada", "ada-password");
    let (ok, out, err) = turso.run_full(&["reindex", "--wipe"]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = turso.run_full(&["users", "list"]);
    assert!(ok && out.contains("ada"), "{out}{err}");

    let Some(pg) = PgScratch::new("acwipe") else {
        return;
    };
    let env = Env::new("acwipe").on_postgres(&pg);
    env.setup_domain("eng");
    users_add(&env, "ada", "ada-password");
    let (ok, out, err) = env.run_full(&["reindex", "--wipe"]);
    assert!(!ok && err.contains("refusing to wipe"), "{out}{err}");
    let (ok, out, err) = env.run_full(&["reindex", "--full"]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(ok && out.contains("ada"), "{out}{err}");
}

/// On Turso `users` writes the `web-auth.db` file in the state directory,
/// as it always did, and a running daemon signs the account in.
#[test]
fn users_cli_unchanged_on_turso() {
    let env = Env::new("acturso");
    users_add(&env, "ada", "ada-password");
    assert!(env.state_dir().join("web-auth.db").exists());
    let (ok, out, err) = env.run_full(&["users", "list"]);
    assert!(ok && out.contains("ada"), "{out}{err}");
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    users_add(&env, "bob", "bob-password");
    assert_eq!(sign_in(&serve, "bob", "bob-password"), 200);
}

/// The start logs one durable-state line: the owner, the database with its
/// schema and where the accounts live, and no credential.
#[test]
fn startup_line_names_owner_and_accounts() {
    let Some(pg) = PgScratch::new("dsline") else {
        return;
    };
    let env = Env::new("dsline").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let log = serve.log();
    let line = log
        .lines()
        .find(|l| l.contains("durable state: owner of postgres"))
        .unwrap_or_else(|| panic!("no durable state line:\n{log}"));
    assert!(
        line.contains(&format!("schema {}", pg.schema)) && line.contains("accounts in postgres"),
        "{line}"
    );
    assert!(line.contains(" since "), "{line}");
    assert!(
        line.contains("configuration document (") && line.contains("config.yaml "),
        "{line}"
    );
    let credentials = pg
        .url
        .split('@')
        .next()
        .and_then(|head| head.split("//").nth(1))
        .unwrap();
    assert!(
        !log.contains(credentials),
        "the log carries the credentials"
    );
}

/// Without a daemon of its own, `status` on an owned database names the
/// owner from the owner row and where the accounts live, in text and JSON.
#[test]
fn status_without_daemon_reports_owner_row() {
    let Some(pg) = PgScratch::new("dsstat") else {
        return;
    };
    let active_env = Env::new("dsstata").on_postgres(&pg);
    let mut active = Serve::start(&active_env, None);
    active.wait_ready();
    let label = owner_label(&pg);

    let other = Env::new("dsstatb").on_postgres(&pg);
    let (ok, out, err) = other.run_full(&["status"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("Ownership and durable state:"), "{out}");
    assert!(out.contains(&format!("Owner: {label}")), "{out}");
    assert!(out.contains("Accounts: in postgres"), "{out}");
    let (ok, out, err) = other.run_full(&["status", "--json"]);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(status["durable_state"]["owner"]["label"], label);
    assert_eq!(status["durable_state"]["accounts"], "postgres");
    assert_eq!(status["durable_state"]["role"], "none");
    assert!(!out.contains(&url_credentials(&pg)));
    assert!(active.alive());
}

/// `doctor --fix` changes nothing while an instance of another machine owns
/// the database, and a plain `doctor` shows the block.
#[test]
fn doctor_fix_refused_when_owned() {
    let Some(pg) = PgScratch::new("dsfix") else {
        return;
    };
    let active_env = Env::new("dsfixa").on_postgres(&pg);
    let mut active = Serve::start(&active_env, None);
    active.wait_ready();
    let label = owner_label(&pg);

    let other = Env::new("dsfixb").on_postgres(&pg);
    let (ok, out, err) = other.run_full(&["doctor", "--fix"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("is owned by"), "{out}{err}");
    let (_, out, err) = other.run_full(&["doctor"]);
    let text = format!("{out}{err}");
    assert!(text.contains("ownership and durable state:"), "{text}");
    assert!(text.contains(&format!("owner: {label}")), "{text}");
    assert!(!text.contains(&url_credentials(&pg)), "{text}");
}

/// An accounts file left on this disk is named by `doctor` on Postgres, and
/// is not read.
#[cfg(feature = "postgres")]
#[test]
fn doctor_warns_local_disk_rows_on_postgres() {
    let Some(pg) = PgScratch::new("dswarn") else {
        return;
    };
    let turso = Env::new("dswarn");
    users_add(&turso, "old", "old-password");
    let leftover = turso.state_dir().join("web-auth.db");
    assert!(leftover.exists());
    let env = turso.on_postgres(&pg);
    let (_, out, err) = env.run_full(&["doctor"]);
    let text = format!("{out}{err}");
    assert!(
        text.contains(&format!(
            "the accounts file {} is not read",
            leftover.display()
        )),
        "{text}"
    );
    assert_eq!(schema_objects(&pg), "0", "doctor opened the store");
}

/// On Turso neither `status` nor `doctor` carries the block.
#[test]
fn status_block_absent_on_turso() {
    let env = Env::new("dsturso");
    let (ok, out, err) = env.run_full(&["status", "--json"]);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert!(status.get("durable_state").is_none(), "{out}");
    let (_, out, err) = env.run_full(&["status"]);
    assert!(
        !format!("{out}{err}").contains("durable state"),
        "{out}{err}"
    );
    let (_, out, err) = env.run_full(&["doctor"]);
    assert!(
        !format!("{out}{err}").contains("durable state"),
        "{out}{err}"
    );
}

/// The credentials of `pg`'s URL, `user:password`.
fn url_credentials(pg: &PgScratch) -> String {
    pg.url
        .split('@')
        .next()
        .and_then(|head| head.split("//").nth(1))
        .unwrap()
        .to_string()
}

/// A daemonless `status` on a free database reports no owner, succeeds and
/// opens nothing.
#[cfg(feature = "postgres")]
#[test]
fn status_on_a_free_database_says_free() {
    let Some(pg) = PgScratch::new("dsfree") else {
        return;
    };
    let env = Env::new("dsfree").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["status"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("Owner: none"), "{out}");
    let (ok, out, err) = env.run_full(&["status", "--json"]);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(status["durable_state"]["owner_state"], "free");
    assert!(!format!("{out}{err}").contains(&url_credentials(&pg)));
    assert_eq!(schema_objects(&pg), "0", "status opened the store");
}

// --- the configuration document (spec section C) -----------------------------

/// The effective value of setting `key` in `config show --json`, as `env`
/// sees it, or panics with the output.
pub(crate) fn shown_setting(env: &Env, key: &str) -> String {
    let (ok, out, err) = env.run_full(&["--json", "config", "show"]);
    assert!(ok, "{out}{err}");
    let shown: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    shown["settings"]
        .as_array()
        .and_then(|views| views.iter().find(|v| v["key"] == key))
        .and_then(|v| v["value"].as_str())
        .unwrap_or_else(|| panic!("no {key} in {out}{err}"))
        .to_string()
}

/// With a daemon owning the database, `domain add` registers through it: the
/// domain lands in the configuration document, config.yaml is never
/// written, and the daemon serves the domain at once.
#[test]
fn domain_add_via_ctl_lands_in_document() {
    let Some(pg) = PgScratch::new("cdctl") else {
        return;
    };
    let env = Env::new("cdctl").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let dir = env.domain_folder("notes");
    // A standalone registration would be refused here: the daemon owns the
    // database, so success means the daemon registered it.
    let (ok, out, err) = env.run_full(&["domain", "add", "notes", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_domains(), "notes");
    assert!(
        !env.config_path().exists(),
        "a Postgres registration wrote config.yaml"
    );
    let (ok, out, err) = env.run_full(&["--json", "search", "seed body token"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("seed"), "{out}{err}");
    // A virtual domain takes the same way.
    let (ok, out, err) = env.run_full(&["domain", "add", "--virtual", "scratch"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_domains(), "notes,scratch");
    assert!(!env.config_path().exists());
    assert!(serve.alive(), "{}", serve.log());
}

/// Without a daemon on a free database, `domain add` takes the database for
/// the command, reads the document and saves the registration there, never
/// in config.yaml. A daemonless reindex then finds the domain in the
/// document.
#[cfg(feature = "postgres")]
#[test]
fn domain_add_standalone_lands_in_document() {
    let Some(pg) = PgScratch::new("cdalone") else {
        return;
    };
    let env = Env::new("cdalone").on_postgres(&pg);
    let dir = env.domain_folder("notes");
    let (ok, out, err) = env.run_full(&["domain", "add", "notes", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_domains(), "notes");
    assert!(
        !env.config_path().exists(),
        "a Postgres registration wrote config.yaml"
    );
    assert_eq!(owner_command(&pg), "crystalline domain add|true");
    let (ok, out, err) = env.run_full(&["domain", "add", "--virtual", "scratch"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_domains(), "notes,scratch");
    let (ok, out, err) = env.run_full(&["reindex"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("notes"), "{out}{err}");
    assert!(!env.config_path().exists());
}

/// Without a daemon on a free database, `config set` takes the database for
/// the command and saves the setting in the document; config.yaml stays
/// untouched and the database is released afterwards.
#[cfg(feature = "postgres")]
#[test]
fn configure_set_without_daemon_writes_document_when_free() {
    let Some(pg) = PgScratch::new("cdset") else {
        return;
    };
    let env = Env::new("cdset").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_value("{service,response_format}"), "json");
    assert!(!env.config_path().exists(), "config set wrote config.yaml");
    assert!(
        owner_command(&pg).ends_with("|true"),
        "{}",
        owner_command(&pg)
    );
    let (ok, out, err) = env.run_full(&["config", "unset", "service.response_format"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_value("{service,response_format}"), "");
    // A `database.*` key is config.yaml's, never the document's.
    let (ok, out, err) = env.run_full(&["config", "set", "database.schema", &pg.schema]);
    assert!(ok, "{out}{err}");
    assert!(
        std::fs::read_to_string(env.config_path())
            .unwrap()
            .contains(&pg.schema)
    );
    assert_eq!(pg.document_value("{database}"), "");
}

/// `update auto` writes where `config set update.check` writes: on
/// Postgres the configuration document, with config.yaml untouched.
#[cfg(feature = "postgres")]
#[test]
fn update_auto_writes_the_document_like_config_set() {
    let Some(pg) = PgScratch::new("cdauto") else {
        return;
    };
    let env = Env::new("cdauto").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["update", "auto", "on"]);
    assert!(ok, "{out}{err}");
    assert!(out.starts_with("Update check on."), "{out}{err}");
    assert_eq!(pg.document_value("{update,check}"), "true");
    assert!(!env.config_path().exists(), "update auto wrote config.yaml");
    let (ok, out, err) = env.run_full(&["update", "auto", "off"]);
    assert!(ok, "{out}{err}");
    assert!(out.starts_with("Update check off."), "{out}{err}");
    assert_eq!(pg.document_value("{update,check}"), "false");
}

/// Without a daemon of its own, `config show` reads the document over a
/// lock-free connection: it shows what the active instance of another
/// machine saved while that instance keeps the database.
#[test]
fn config_show_without_daemon_reads_document_lock_free() {
    let Some(pg) = PgScratch::new("cdshow") else {
        return;
    };
    let owner_env = Env::new("cdshowa").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let (ok, out, err) = owner_env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_value("{service,response_format}"), "json");

    let env = Env::new("cdshowb").on_postgres(&pg);
    assert_eq!(shown_setting(&env, "service.response_format"), "json");
    assert_eq!(
        owner_command(&pg),
        "|false",
        "config show took the database"
    );
    assert_eq!(owner.get("/ready").unwrap().status().as_u16(), 200);
}

/// A TCP listener that counts the connections it accepts and drops each at
/// once: a database address any Postgres connect reaches and fails on.
struct CountingListener {
    url: String,
    accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl CountingListener {
    fn start() -> CountingListener {
        // Never the port of the local test Postgres: a listener there would
        // sit in front of the real database and break every other test.
        let test_port = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
            .ok()
            .and_then(|url| {
                let rest = url.split_once("://")?.1;
                let host = rest.split(['/', '?']).next()?.rsplit('@').next()?;
                host.rsplit_once(':')?.1.parse::<u16>().ok()
            });
        let (listener, port) = loop {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            if Some(port) != test_port {
                break (listener, port);
            }
        };
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = accepted.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                drop(stream);
            }
        });
        CountingListener {
            url: format!("postgres://nobody:s3cretpw@127.0.0.1:{port}/nothing"),
            accepted,
        }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A `crystalline mcp` bridge that reaches a daemon never reads the
/// document itself: its database address is a listener of the test, which
/// accepts no connection over the bridge's whole session (an initialize, a
/// tools/list and a pause), while the daemon serves from the real database.
/// A daemonless `config show` with the same address does reach the
/// listener, so the count is a real witness.
#[test]
fn mcp_bridge_reaching_daemon_never_fetches_document() {
    let Some(pg) = PgScratch::new("cdbridge") else {
        return;
    };
    let env = Env::new("cdbridge").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let watch = CountingListener::start();
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    let log = env.state_dir().with_file_name("bridge.log");
    let mut bridge = cmd
        .env("CRYSTALLINE_DATABASE_URL", &watch.url)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let mut stdin = bridge.stdin.take().unwrap();
    let stdout = bridge.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match std::io::BufRead::read_line(&mut reader, &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let err = || std::fs::read_to_string(&log).unwrap_or_default();
    let ask = |stdin: &mut std::process::ChildStdin, request: Value| -> Value {
        writeln!(stdin, "{request}").unwrap();
        let line = rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("the bridge did not answer: {}", err()));
        serde_json::from_str(line.trim()).unwrap_or(Value::Null)
    };
    let reply = ask(
        &mut stdin,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "it", "version": "0" },
            },
        }),
    );
    assert!(reply["result"].is_object(), "{reply} {}", err());
    writeln!(
        stdin,
        "{}",
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();
    let reply = ask(
        &mut stdin,
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    assert!(reply["result"]["tools"].is_array(), "{reply} {}", err());
    std::thread::sleep(Duration::from_secs(2));
    drop(stdin);
    let _ = bridge.wait();
    assert_eq!(
        watch.accepted(),
        0,
        "the bridge reached the database: {}",
        err()
    );
    assert!(serve.alive(), "{}", serve.log());

    // The witness: a command without a daemon does reach the address.
    let other = Env::new("cdbridgeb").on_postgres(&pg);
    let mut show = Command::new(bin());
    other.apply(&mut show);
    let out = show
        .env("CRYSTALLINE_DATABASE_URL", &watch.url)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(watch.accepted() > 0, "the listener saw no connect at all");
}

/// A report never fails on the configuration document: when it cannot be
/// read, `status` and `domain list` warn on stderr that the registrations
/// shown are config.yaml's alone, and still answer.
#[test]
fn a_report_warns_and_answers_when_the_document_cannot_be_read() {
    let Some(pg) = PgScratch::new("cdwarn") else {
        return;
    };
    let env = Env::new("cdwarn").on_postgres(&pg);
    let watch = CountingListener::start();
    for args in [&["status", "--json"][..], &["domain", "list", "--json"][..]] {
        let mut cmd = Command::new(bin());
        env.apply(&mut cmd);
        let out = cmd
            .env("CRYSTALLINE_DATABASE_URL", &watch.url)
            .args(args)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            stderr.contains("the registrations shown are config.yaml's alone"),
            "{args:?}: no warning: {stderr}"
        );
        assert!(
            stderr.contains("could not read the configuration in the database"),
            "{args:?}: {stderr}"
        );
        assert!(
            !stderr.contains("s3cretpw")
                && !String::from_utf8_lossy(&out.stdout).contains("s3cretpw"),
            "{args:?} leaked the password: {stderr}"
        );
        assert!(
            serde_json::from_slice::<Value>(&out.stdout).is_ok(),
            "{args:?} did not answer: {} {stderr}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    assert!(watch.accepted() > 0, "the listener saw no connect at all");
}

/// A takeover that cannot read the configuration document stops the start
/// with a sentence that says so, and leaves the database free: the instance
/// never goes on with config.yaml alone.
#[cfg(feature = "postgres")]
#[test]
fn unreachable_document_at_takeover_fails_start() {
    let Some(pg) = PgScratch::new("cdgone") else {
        return;
    };
    let env = Env::new("cdgone").on_postgres(&pg);
    let mut first = Serve::start(&env, None);
    first.wait_ready();
    first.terminate();
    assert_eq!(first.wait_exit(Duration::from_secs(15)), Some(0));
    // The table is gone while the migration ledger says it is there, so the
    // store open does not make it again and the read fails.
    assert!(pg.sql(&format!("DROP TABLE {}.instance_config", pg.schema)));

    let mut serve = Serve::start(&env, None);
    let code = serve.wait_exit(Duration::from_secs(30));
    assert!(
        matches!(code, Some(c) if c != 0),
        "{code:?}: {}",
        serve.log()
    );
    let log = serve.log();
    assert!(
        log.contains("could not read the configuration in the database")
            && log.contains("does not start without it"),
        "{log}"
    );
    let key = crystalline_index::postgres::owner_key(Some(&pg.schema));
    assert_eq!(
        pg.text(&format!(
            "SELECT (count(*) = 0)::text FROM pg_locks WHERE locktype = 'advisory' \
             AND objid::bigint = ({key} & 4294967295)"
        )),
        "true",
        "the database is still held"
    );
}

/// A configuration document that does not read as a configuration stops
/// the start too: the instance never saves an empty document over it.
#[cfg(feature = "postgres")]
#[test]
fn invalid_document_at_takeover_fails_start() {
    let Some(pg) = PgScratch::new("cdbad") else {
        return;
    };
    let env = Env::new("cdbad").on_postgres(&pg);
    let mut first = Serve::start(&env, None);
    first.wait_ready();
    first.terminate();
    assert_eq!(first.wait_exit(Duration::from_secs(15)), Some(0));
    assert!(pg.sql(&format!(
        "INSERT INTO {}.instance_config (id, body, updated_at, updated_by) \
         VALUES (1, '{{\"domains\": 7}}', now(), 'test') \
         ON CONFLICT (id) DO UPDATE SET body = excluded.body",
        pg.schema
    )));

    let mut serve = Serve::start(&env, None);
    let code = serve.wait_exit(Duration::from_secs(30));
    assert!(
        matches!(code, Some(c) if c != 0),
        "{code:?}: {}",
        serve.log()
    );
    let log = serve.log();
    assert!(
        log.contains("could not read the configuration in the database") && log.contains("invalid"),
        "{log}"
    );
    assert_eq!(
        pg.document_value("{domains}"),
        "7",
        "the document was replaced"
    );
}

/// `service.allowed_hosts` saved in the document applies after a redeploy:
/// the router, which is built after the takeover read, answers the host,
/// and the ctl status and the ctl holder answer say it. Another host is
/// still refused.
#[test]
fn config_allowed_hosts_from_document_apply_after_redeploy() {
    let Some(pg) = PgScratch::new("cdhosts") else {
        return;
    };
    let env = Env::new("cdhosts").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["config", "set", "service.allowed_hosts", "kb.example"]);
    assert!(ok, "{out}{err}");
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
    std::fs::remove_dir_all(env.state_dir()).unwrap();

    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["ctl", "status", "--json"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("kb.example"), "{out}");
    #[cfg(unix)]
    {
        let holder = crate::service::ctl_over_the_socket(
            &env,
            &serde_json::json!({ "v": 1, "cmd": "holder" }),
        );
        assert!(
            holder["data"]["allowed_hosts"]
                .as_array()
                .is_some_and(|hosts| hosts.iter().any(|h| h == "kb.example")),
            "{holder}"
        );
    }
    let ask = |host: &str| {
        reqwest::blocking::Client::new()
            .get(format!("http://{}/api/v1/me", serve.addr))
            .header("Host", host)
            .timeout(Duration::from_secs(5))
            .send()
            .unwrap()
    };
    let refused = ask("evil.example");
    assert_eq!(refused.status().as_u16(), 403);
    let answered = ask("kb.example");
    let status = answered.status().as_u16();
    let body = answered.text().unwrap_or_default();
    assert!(
        !body.contains("Host is not one this instance answers to"),
        "{status}: {body}"
    );
}

/// `service.http` is bound before the takeover reads the document, so a
/// value saved there could never apply: `config set service.http` is
/// refused in document mode, through the daemon and without one, and names
/// where the value belongs.
#[cfg(feature = "postgres")]
#[test]
fn config_set_service_http_refused_in_document_mode() {
    let Some(pg) = PgScratch::new("cdhttp") else {
        return;
    };
    let alone = Env::new("cdhttpb").on_postgres(&pg);
    let (ok, out, err) = alone.run_full(&["config", "set", "service.http", "0.0.0.0:7411"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SERVICE_HTTP"), "{err}");
    assert_eq!(schema_objects(&pg), "0", "the refusal opened the database");

    let env = Env::new("cdhttp").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["config", "set", "service.http", "0.0.0.0:7411"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SERVICE_HTTP"), "{err}");
    assert_eq!(pg.document_value("{service,http}"), "");
}

/// A `service.public_url` with a path would split the base path between
/// the early probes and the full router, so a set of one is refused in
/// document mode, through the daemon and without one, naming where it
/// belongs; nothing reaches the document.
#[cfg(feature = "postgres")]
#[test]
fn config_set_public_url_with_path_refused_in_document_mode() {
    let Some(pg) = PgScratch::new("cdpub") else {
        return;
    };
    let url = "https://kb.example/crystalline";
    let alone = Env::new("cdpubb").on_postgres(&pg);
    let (ok, out, err) = alone.run_full(&["config", "set", "service.public_url", url]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SERVICE_PUBLIC_URL"), "{err}");
    assert_eq!(schema_objects(&pg), "0", "the refusal opened the database");

    let env = Env::new("cdpub").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["config", "set", "service.public_url", url]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SERVICE_PUBLIC_URL"), "{err}");
    assert_eq!(pg.document_value("{service,public_url}"), "");
}

/// `service.read_only` saved in the document does not lock the settings:
/// `config unset service.read_only` takes it out again with no environment
/// override, without a daemon and through a read-only daemon.
#[cfg(feature = "postgres")]
#[test]
fn read_only_in_document_can_be_unset() {
    let Some(pg) = PgScratch::new("cdro") else {
        return;
    };
    let env = Env::new("cdro").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["config", "set", "service.read_only", "true"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_value("{service,read_only}"), "true");
    let (ok, out, err) = env.run_full(&["config", "unset", "service.read_only"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_value("{service,read_only}"), "");

    let (ok, out, err) = env.run_full(&["config", "set", "service.read_only", "true"]);
    assert!(ok, "{out}{err}");
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["config", "unset", "service.read_only"]);
    assert!(ok, "{out}{err}");
    assert_eq!(pg.document_value("{service,read_only}"), "");
}

/// Without a daemon of its own, `domain list`, `status` and `doctor` read
/// the registrations from the document lock-free, while the active
/// instance of another machine keeps the database.
#[test]
fn reports_without_daemon_read_document_registrations() {
    let Some(pg) = PgScratch::new("cdlist") else {
        return;
    };
    let owner_env = Env::new("cdlista").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let dir = owner_env.domain_folder("notes");
    let (ok, out, err) = owner_env.run_full(&["domain", "add", "notes", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");

    let env = Env::new("cdlistb").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["--json", "domain", "list"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("notes"), "{out}{err}");
    let (_, out, err) = env.run_full(&["status", "--json"]);
    let status: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
    assert_eq!(
        status["registered"],
        serde_json::json!(["notes"]),
        "{out}{err}"
    );
    let (_, out, err) = env.run_full(&["doctor"]);
    assert!(format!("{out}{err}").contains("notes"), "{out}{err}");
    assert_eq!(owner_command(&pg), "|false", "a report took the database");
    assert!(owner.alive());
}

/// A redeploy keeps the configuration: a setting and a domain saved through
/// the daemon survive a stop and a start with fresh state and config
/// folders, because they live in the database.
#[test]
fn config_survives_redeploy() {
    let Some(pg) = PgScratch::new("cdredeploy") else {
        return;
    };
    let env = Env::new("cdredeploy").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(ok, "{out}{err}");
    let dir = env.domain_folder("notes");
    let (ok, out, err) = env.run_full(&["domain", "add", "notes", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));

    std::fs::remove_dir_all(env.state_dir()).unwrap();
    let config_dir = env.config_path().parent().unwrap().to_path_buf();
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir).unwrap();
    }
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    assert_eq!(shown_setting(&env, "service.response_format"), "json");
    let (ok, out, err) = env.run_full(&["--json", "search", "seed body token"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("seed"), "{out}{err}");
    assert!(!env.config_path().exists());
}

/// On Turso nothing moves: without a daemon `config set` and `domain add`
/// write config.yaml as they always did.
#[test]
fn config_writes_unchanged_on_turso() {
    let env = Env::new("cdturso");
    let (ok, out, err) = env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(ok, "{out}{err}");
    let dir = env.domain_folder("notes");
    let (ok, out, err) = env.run_full(&["domain", "add", "notes", dir.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    let cfg: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&env.config_path()).unwrap();
    assert_eq!(
        cfg.service.and_then(|s| s.response_format),
        Some(crystalline_core::config::ResponseFormat::Json)
    );
    assert!(cfg.domains.contains_key("notes"), "{:?}", cfg.domains);
}

/// An index that does not open is described for its backend. On Postgres
/// (here nothing listens on the database's port) `reindex` names the
/// database and the checks that fit it: the server, the sign-in and the
/// connection limit, never the file advice or the `--wipe` hint. On Turso
/// (here the index path is a folder) the file advice and the hint stay.
/// Needs no Postgres server.
#[test]
fn an_index_that_does_not_open_gets_the_advice_of_its_backend() {
    let env = Env::new("pgdown");
    let url = format!("postgres://u:p@127.0.0.1:{}/nodb", free_port());
    let vars = [
        ("CRYSTALLINE_DATABASE_BACKEND", "postgres"),
        ("CRYSTALLINE_DATABASE_URL", url.as_str()),
        ("CRYSTALLINE_DATABASE_SCHEMA", "down"),
        ("CRYSTALLINE_SECRET_KEY", crate::service::TEST_SECRET_KEY),
    ];
    let (ok, out, err) = run_vars(&env, &["reindex"], &vars);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("schema down could not be opened"), "{err}");
    assert!(err.contains("Postgres server is running"), "{err}");
    assert!(err.contains("max_connections"), "{err}");
    assert!(!err.contains("file is readable"), "{err}");
    assert!(!err.contains("reindex --wipe"), "{err}");

    // A password with a raw `?` makes a URL that cannot be read safely: the
    // words name the schema but no part of the URL.
    let url = format!("postgres://u:pq?zw@127.0.0.1:{}/nodb", free_port());
    let mut odd = vars;
    odd[1] = ("CRYSTALLINE_DATABASE_URL", url.as_str());
    let (ok, out, err) = run_vars(&env, &["reindex"], &odd);
    assert!(!ok, "{out}{err}");
    assert!(
        err.contains(
            "the index in the database (address not shown), schema down could not be opened"
        ),
        "{err}"
    );
    assert!(!err.contains("pq") && !err.contains("zw"), "{err}");

    let turso = Env::new("filedown");
    std::fs::create_dir_all(turso.state_dir().join("index.db/not-a-file")).unwrap();
    let (ok, out, err) = turso.run_full(&["reindex"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("Check that the file is readable"), "{err}");
    assert!(err.contains("crystalline reindex --wipe"), "{err}");
    assert!(!err.contains("Postgres server"), "{err}");
}

// --- the CLI credential writers on Postgres (spec section D) -----------------

/// Run `crystalline <args>` in `env` with `extra` variables on top of its
/// own: success, stdout and stderr.
pub(crate) fn run_vars(env: &Env, args: &[&str], extra: &[(&str, &str)]) -> (bool, String, String) {
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
    for (key, value) in extra {
        cmd.env(key, value);
    }
    let out = cmd.args(args).stdin(Stdio::null()).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Seal a GitHub token for `user` into the row `name` of `pg`'s
/// `instance_secret`, with the key every Postgres test process runs with.
#[cfg(feature = "postgres")]
fn seal_token_row(pg: &PgScratch, name: &str, user: &str) {
    use crystalline_remote::StoredToken;
    use crystalline_remote::sealed::{SealKey, seal};

    use crate::service::TEST_SECRET_KEY;

    let token = StoredToken {
        access_token: format!("{user}-secret"),
        host: "github.com".to_string(),
        user: user.to_string(),
        created_at: crystalline_core::time::now(),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    };
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&TEST_SECRET_KEY[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    let key = SealKey::from_bytes(bytes.try_into().unwrap());
    let sealed = seal(&key, name, &serde_json::to_vec(&token).unwrap());
    let hex: String = sealed.iter().map(|b| format!("{b:02x}")).collect();
    pg.run(&format!(
        "INSERT INTO {}.instance_secret VALUES ('{name}', decode('{hex}', 'hex'), '{}', now())",
        pg.schema,
        key.key_id()
    ))
    .unwrap();
}

/// Whether `pg`'s `instance_secret` holds the row `name`.
#[cfg(feature = "postgres")]
pub(crate) fn has_secret_row(pg: &PgScratch, name: &str) -> bool {
    pg.text(&format!(
        "SELECT count(*)::text FROM {}.instance_secret WHERE name = '{name}'",
        pg.schema
    )) == "1"
}

/// How many accounts named `name` `pg` holds, as text.
#[cfg(feature = "postgres")]
fn accounts_named(pg: &PgScratch, name: &str) -> String {
    pg.text(&format!(
        "SELECT count(*)::text FROM {}.users WHERE name = '{name}'",
        pg.schema
    ))
}

/// The row a personal GitHub token of `name` is sealed under, on github.com.
#[cfg(feature = "postgres")]
fn personal_row(name: &str) -> String {
    format!("github-personal:{name}")
}

/// No process without the key touches the vault: with no daemon of this
/// machine, `users remove`, `users disable` and `users add` are refused
/// before any write, naming `kubectl exec` and the key variable. The account
/// and its sealed token both stay: nothing is half done.
#[cfg(feature = "postgres")]
#[test]
fn users_remove_without_key_refused_before_any_write() {
    let Some(pg) = PgScratch::new("vtnokey") else {
        return;
    };
    let env = Env::new("vtnokey").on_postgres(&pg);
    users_add(&env, "bob", "bob-password");
    let row = personal_row("bob");
    seal_token_row(&pg, &row, "bob");
    for args in [
        &["users", "remove", "bob"][..],
        &["users", "remove", "bob", "--force"],
        &["users", "disable", "bob"],
    ] {
        let (ok, out, err) = run_vars(&env, args, &NO_SECRET_KEY);
        assert!(!ok, "{out}{err}");
        assert!(
            err.contains(&format!(
                "for example `kubectl exec <pod> -- crystalline {}`",
                args.join(" ")
            )),
            "{err}"
        );
        assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
        assert_eq!(accounts_named(&pg, "bob"), "1", "the account is kept");
        assert!(has_secret_row(&pg, &row), "the sealed token is kept");
    }
    let disabled = pg.text(&format!(
        "SELECT disabled::text FROM {}.users WHERE name = 'bob'",
        pg.schema
    ));
    assert_eq!(disabled, "0", "nothing was disabled");
    let (ok, out, err) = run_vars(
        &env,
        &["users", "add", "carol", "--mcp-token", "agent"],
        &NO_SECRET_KEY,
    );
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
    assert_eq!(accounts_named(&pg, "carol"), "0", "no account was made");
}

/// With no daemon and a free database, `users remove` with the key claims
/// the database for the command, deletes the sealed token and the account,
/// and gives the database back.
#[cfg(feature = "postgres")]
#[test]
fn users_remove_takes_lock_when_free_and_deletes_row() {
    let Some(pg) = PgScratch::new("vtfree") else {
        return;
    };
    let env = Env::new("vtfree").on_postgres(&pg);
    users_add(&env, "bob", "bob-password");
    let row = personal_row("bob");
    seal_token_row(&pg, &row, "bob");
    seal_token_row(&pg, "github", "machine");
    let (ok, out, err) = env.run_full(&["users", "remove", "bob"]);
    assert!(ok, "{out}{err}");
    assert_eq!(accounts_named(&pg, "bob"), "0");
    assert!(!has_secret_row(&pg, &row), "the sealed token is deleted");
    assert!(has_secret_row(&pg, "github"), "another row is kept");
    assert_eq!(
        owner_command(&pg),
        "crystalline users remove|true",
        "the command held the database and gave it back"
    );
}

/// With a daemon of this machine owning the database, `users remove` goes
/// through it: the daemon deletes the sealed token (`forget_credential` with
/// `"delete": true`), the command needs no key of its own, and the owner row
/// stays the daemon's.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn users_remove_deletes_vault_row_via_daemon() {
    let Some(pg) = PgScratch::new("vtdaemon") else {
        return;
    };
    let env = Env::new("vtdaemon").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let (ok, out, err) = run_with_stdin(
        &env,
        &["users", "add", "bob", "--password-stdin"],
        "bob-password\n",
    );
    assert!(ok, "{out}{err}");
    let row = personal_row("bob");
    seal_token_row(&pg, &row, "bob");
    // The daemon answers the delete for a person, and for this machine.
    let reply = crate::service::ctl_over_the_socket(
        &env,
        &serde_json::json!({ "cmd": "forget_credential", "account": "nobody", "delete": true }),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["data"]["forgotten"], "nobody", "{reply}");
    assert_eq!(reply["data"]["deleted"], false, "{reply}");

    let (ok, out, err) = run_vars(&env, &["users", "remove", "bob"], &NO_SECRET_KEY);
    assert!(ok, "{out}{err}");
    assert_eq!(accounts_named(&pg, "bob"), "0");
    assert!(!has_secret_row(&pg, &row), "the daemon deleted the row");
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );
    assert!(serve.alive());
}

/// While another instance owns the database, `users add` with the key and
/// no daemon of this machine is refused before the account is written,
/// with the sentence naming the owner and `kubectl exec`.
#[cfg(feature = "postgres")]
#[test]
fn users_add_refused_before_auth_write_when_owned_elsewhere() {
    let Some(pg) = PgScratch::new("vtelse") else {
        return;
    };
    let owner_env = Env::new("vtelsea").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let label = owner_label(&pg);

    let env = Env::new("vtelseb").on_postgres(&pg);
    let (ok, out, err) = run_with_stdin(
        &env,
        &["users", "add", "carol", "--password-stdin"],
        "carol-password\n",
    );
    assert!(!ok, "{out}{err}");
    assert!(err.contains("sealed in the database"), "{err}");
    assert!(
        err.contains(&format!("is owned by the active instance {label}")),
        "{err}"
    );
    assert!(
        err.contains("kubectl exec <pod> -- crystalline users add carol"),
        "{err}"
    );
    assert_eq!(accounts_named(&pg, "carol"), "0", "no account was made");
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );
}

/// The `users` subcommands that change no secret write the accounts beside
/// the owner as before, with no key and no daemon of this machine: `link`
/// and `unlink` work while another instance owns the database.
#[cfg(feature = "postgres")]
#[test]
fn users_unlink_unaffected_by_vault() {
    let Some(pg) = PgScratch::new("vtunlink") else {
        return;
    };
    let owner_env = Env::new("vtunlinka").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    users_add(&owner_env, "bob", "bob-password");

    let env = Env::new("vtunlinkb").on_postgres(&pg);
    let issuer = "https://idp.example";
    let (ok, out, err) = run_vars(
        &env,
        &[
            "users",
            "link",
            "bob",
            "--issuer",
            issuer,
            "--subject",
            "s-1",
        ],
        &NO_SECRET_KEY,
    );
    assert!(ok, "{out}{err}");
    let (ok, out, err) = run_vars(
        &env,
        &["users", "unlink", "bob", "--issuer", issuer],
        &NO_SECRET_KEY,
    );
    assert!(ok, "{out}{err}");
    assert!(out.contains("Unlinked 'bob'"), "{out}");
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );
}

/// `connect github` with no daemon of this machine never writes a token to
/// the state folder on Postgres: refused before any sign-in while another
/// instance owns the database, and without the key on a free one. Nothing
/// appears under `origins/`.
#[cfg(feature = "postgres")]
#[test]
fn connect_github_without_owner_refused_not_written_locally() {
    let Some(pg) = PgScratch::new("vtconn") else {
        return;
    };
    let env = Env::new("vtconn").on_postgres(&pg);
    let origins = env.state_dir().join("origins");
    let args = ["connect", "github", "--token", "ghp_notatoken"];
    let (ok, out, err) = run_vars(&env, &args, &NO_SECRET_KEY);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
    assert!(
        err.contains("kubectl exec <pod> -- crystalline connect github --token <value>"),
        "{err}"
    );
    assert!(
        !err.contains("ghp_notatoken"),
        "the token is never repeated"
    );

    let owner_env = Env::new("vtconna").on_postgres(&pg);
    let mut owner = Serve::start(&owner_env, None);
    owner.wait_ready();
    let (ok, out, err) = env.run_full(&args);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("sealed in the database"), "{err}");
    assert!(err.contains("is owned by the active instance"), "{err}");
    let written: Vec<_> = std::fs::read_dir(&origins)
        .map(|dir| dir.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(written.is_empty(), "written locally: {written:?}");
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );
}

/// `crystalline mcp --embedded` on Postgres without the key is refused with
/// the daemon's sentence as the initialize error, and takes nothing.
#[cfg(feature = "postgres")]
#[test]
fn embedded_refused_without_key() {
    let Some(pg) = PgScratch::new("vtembed") else {
        return;
    };
    let env = Env::new("vtembed").on_postgres(&pg);
    let (code, out, err) = mcp_once_with(&env, &["mcp", "--embedded"], &NO_SECRET_KEY);
    assert_ne!(code, Some(0), "{out}{err}");
    let message = initialize_error(&out);
    assert!(message.contains("CRYSTALLINE_SECRET_KEY"), "{message}");
    assert!(message.contains("openssl rand -base64 32"), "{message}");
    assert_eq!(schema_objects(&pg), "0", "nothing was created");
}

/// `status` and `doctor` on Postgres report whether a key is configured
/// and how many secrets are sealed, never a row name, and a missing key
/// fails neither.
#[cfg(feature = "postgres")]
#[test]
fn reports_count_sealed_secrets_and_never_need_the_key() {
    let Some(pg) = PgScratch::new("vtreport") else {
        return;
    };
    let env = Env::new("vtreport").on_postgres(&pg);
    users_add(&env, "bob", "bob-password");
    seal_token_row(&pg, &personal_row("bob"), "bob");
    let (ok, out, err) = run_vars(&env, &["status", "--json"], &NO_SECRET_KEY);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    let secrets = &status["durable_state"]["secrets"];
    assert_eq!(secrets["sealed"], 1, "{status}");
    assert_eq!(secrets["key_configured"], false, "{status}");
    assert!(
        !status["durable_state"].to_string().contains(&test_key_id()),
        "a report names no key id: {status}"
    );
    assert_eq!(
        status["durable_state"]["github_app"]["source"], "not_checked",
        "{status}"
    );
    let (ok, out, err) = env.run_full(&["status"]);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("Secrets: 1 sealed in the database; a key is configured"),
        "{out}"
    );
    assert!(
        out.contains("GitHub App: not checked (a saved App would be sealed in the database)"),
        "{out}"
    );
    let (_, out, err) = run_vars(&env, &["doctor"], &NO_SECRET_KEY);
    let text = format!("{out}{err}");
    assert!(
        text.contains("secrets: 1 sealed in the database; no key is configured here"),
        "{text}"
    );
    assert!(
        text.contains("github app: not checked (a saved App would be sealed in the database)"),
        "{text}"
    );
    assert!(!text.contains(&test_key_id()), "a report names no key id");
    // An App from the environment is known without the vault.
    let app = [
        ("CRYSTALLINE_GITHUB_APP_ID", "7"),
        ("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.env"),
        (
            "CRYSTALLINE_GITHUB_APP_PRIVATE_KEY",
            include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem"),
        ),
    ];
    let (ok, out, err) = run_vars(&env, &["status", "--json"], &app);
    assert!(ok, "{out}{err}");
    let status: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        status["durable_state"]["github_app"]["source"], "env",
        "{status}"
    );
    // App variables that do not make an App are an environment error, not
    // "not checked".
    let (_, out, err) = run_vars(
        &env,
        &["status", "--json"],
        &[
            ("CRYSTALLINE_GITHUB_APP_ID", "7"),
            ("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.env"),
            ("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", "not a private key"),
        ],
    );
    let status: Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}{err}"));
    assert_eq!(
        status["durable_state"]["github_app"]["source"], "env_error",
        "{status}"
    );
    for text in [&text, &out] {
        assert!(!text.contains("github-personal"), "a row name: {text}");
    }
}

/// On Turso the credential paths stay as they were: `users add` sweeps a
/// token file left under the name, `users remove` deletes the person's
/// token file, both with a secret key set (it is ignored), and no Postgres
/// is involved.
#[test]
fn credential_paths_unchanged_on_turso() {
    use crystalline_remote::{StoredToken, TokenIdentity, TokenStore};

    let env = Env::new("vtturso");
    let key = [("CRYSTALLINE_SECRET_KEY", "not-a-key")];
    let origins = env.state_dir().join("origins");
    let bob = TokenIdentity::Personal("bob".to_string());
    let file = TokenStore::file_fallback_for(&bob, &origins).unwrap();
    let token = StoredToken {
        access_token: "bob-secret".to_string(),
        host: "github.com".to_string(),
        user: "bob".to_string(),
        created_at: crystalline_core::time::now(),
        refresh_token: None,
        expires_at: None,
        refresh_expires_at: None,
    };
    file.save(&token).unwrap();
    let (ok, out, err) = run_vars(&env, &["users", "add", "bob", "--mcp-token", "agent"], &key);
    assert!(ok, "{out}{err}");
    assert!(
        file.load().unwrap().is_none(),
        "the leftover token went at add"
    );
    file.save(&token).unwrap();
    let (ok, out, err) = run_vars(&env, &["users", "remove", "bob"], &key);
    assert!(ok, "{out}{err}");
    assert!(file.load().unwrap().is_none(), "the token went at remove");
    assert!(env.state_dir().join("web-auth.db").exists());
}

// --- sealing beside the daemon (in the pod) ---------------------------------

/// A stand-in GitHub on a loopback port: the OAuth device flow (its token
/// `ghu_new` for the login `ada-gh`), `GET /user`, and the App routes `set`
/// asks. It records the `Authorization` header of every `GET /user`.
#[cfg(all(unix, feature = "postgres"))]
struct PodGithub {
    url: String,
    users_asked_with: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    _thread: std::thread::JoinHandle<()>,
}

#[cfg(all(unix, feature = "postgres"))]
impl PodGithub {
    fn start() -> PodGithub {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let path = path.split('?').next().unwrap_or("/").to_string();
                let (status, body) = match path.as_str() {
                    "/login/device/code" => (
                        "200 OK",
                        r#"{"device_code":"dc-1","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0}"#.to_string(),
                    ),
                    "/login/oauth/access_token" => (
                        "200 OK",
                        r#"{"access_token":"ghu_new","token_type":"bearer","scope":"repo"}"#
                            .to_string(),
                    ),
                    "/user" => {
                        let auth = request
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                            .map(|l| l.split_once(':').unwrap().1.trim().to_string())
                            .unwrap_or_default();
                        seen.lock().unwrap().push(auth);
                        ("200 OK", r#"{"login":"ada-gh","id":4242}"#.to_string())
                    }
                    "/app" => (
                        "200 OK",
                        r#"{"id":7,"slug":"crystalline-pod","owner":{"login":"acme"},"permissions":{"contents":"write","pull_requests":"write","metadata":"read","members":"read"}}"#.to_string(),
                    ),
                    p if p.starts_with("/app/installations") => (
                        "200 OK",
                        r#"[{"id":1,"account":{"login":"acme"}}]"#.to_string(),
                    ),
                    _ => ("404 Not Found", r#"{"message":"Not Found"}"#.to_string()),
                };
                let date = crystalline_core::time::now().http_date();
                let response = format!(
                    "HTTP/1.1 {status}\r\nDate: {date}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        PodGithub {
            url,
            users_asked_with: asked,
            _thread: thread,
        }
    }

    /// The host part of the URL, which names the rows sealed for it.
    fn host(&self) -> &str {
        self.url.trim_start_matches("http://")
    }
}

/// The variables the daemon and the CLI share in the pod: collaboration
/// on, GitHub at `github`, and an admin for the settings API.
#[cfg(all(unix, feature = "postgres"))]
fn pod_vars(github: &PodGithub) -> Vec<(&'static str, String)> {
    vec![
        ("CRYSTALLINE_GITHUB_ENABLED", "true".to_string()),
        ("CRYSTALLINE_GITHUB_API_URL", github.url.clone()),
        ("CRYSTALLINE_ADMIN_NAME", "root".to_string()),
        ("CRYSTALLINE_ADMIN_PASSWORD", "root-password".to_string()),
    ]
}

/// `GET /api/v1/settings/github` of `serve`, signed in as the admin of
/// [`pod_vars`].
#[cfg(all(unix, feature = "postgres"))]
fn github_settings(serve: &Serve) -> Value {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    let login = client
        .post(format!("http://{}/api/v1/auth/login", serve.addr))
        .json(&serde_json::json!({ "name": "root", "password": "root-password" }))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap();
    assert_eq!(login.status().as_u16(), 200);
    let cookies: Vec<String> = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_string))
        .collect();
    client
        .get(format!("http://{}/api/v1/settings/github", serve.addr))
        .header("cookie", cookies.join("; "))
        .timeout(Duration::from_secs(10))
        .send()
        .unwrap()
        .json()
        .unwrap()
}

/// Every file under the state folder whose name says it holds a GitHub
/// token or App.
#[cfg(all(unix, feature = "postgres"))]
pub(crate) fn local_github_files(env: &Env) -> Vec<PathBuf> {
    fn walk(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("github"))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(&env.state_dir(), &mut found);
    found
}

/// In the pod: `connect github` while the daemon owns the database seals
/// the token itself, beside the daemon, with the key of the shared
/// environment. The row lands in the vault, the daemon drops the token it
/// cached and reads the new one, and nothing is written to the state
/// folder.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn connect_github_in_the_pod_seals_beside_the_daemon() {
    let Some(pg) = PgScratch::new("vtpod") else {
        return;
    };
    let github = PodGithub::start();
    let env = Env::new("vtpod").on_postgres(&pg);
    let vars = pod_vars(&github);
    let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let row = format!("github:{}", github.host());
    let mut serve = Serve::start_bound(&env, "0.0.0.0", Some("info"), &vars);
    serve.wait_ready();
    seal_token_row(&pg, &row, "old-user");
    assert_eq!(github_settings(&serve)["user"], "old-user", "cached");

    let (ok, out, err) = run_vars(&env, &["connect", "github"], &vars);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("Connected to GitHub as ada-gh (vault token store)"),
        "{out}"
    );
    assert!(has_secret_row(&pg, &row));
    assert_eq!(secret_rows(&pg), "1");
    let settings = github_settings(&serve);
    assert_eq!(settings["token_store"], "vault", "{settings}");
    assert_eq!(settings["user"], "ada-gh", "the daemon reads the new token");
    assert!(
        github
            .users_asked_with
            .lock()
            .unwrap()
            .iter()
            .all(|auth| auth.ends_with("ghu_new")),
        "GitHub was asked with the new token only"
    );
    assert_eq!(local_github_files(&env), Vec::<PathBuf>::new());
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );

    // A disconnect in the pod deletes the row the same way, and the daemon
    // is told: it no longer holds the token.
    let (ok, out, err) = run_vars(&env, &["connect", "github", "--disconnect"], &vars);
    assert!(ok, "{out}{err}");
    assert!(!has_secret_row(&pg, &row));
    let settings = github_settings(&serve);
    assert!(settings["user"].is_null(), "{settings}");
    assert!(serve.alive());
}

/// In the pod: `github app set` and `github app remove` seal and delete the
/// App beside the daemon, which picks the change up at once; nothing is
/// written to the state folder.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn github_app_set_in_the_pod_seals_beside_the_daemon() {
    let Some(pg) = PgScratch::new("vtpodapp") else {
        return;
    };
    let github = PodGithub::start();
    let env = Env::new("vtpodapp").on_postgres(&pg);
    let vars = pod_vars(&github);
    let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut serve = Serve::start_bound(&env, "0.0.0.0", Some("info"), &vars);
    serve.wait_ready();
    let key = env.state_dir().join("../app.pem");
    std::fs::write(
        &key,
        include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem"),
    )
    .unwrap();
    let (ok, out, err) = run_vars(
        &env,
        &[
            "github",
            "app",
            "set",
            "--app-id",
            "7",
            "--client-id",
            "Iv23.pod",
            "--private-key-file",
            key.to_str().unwrap(),
        ],
        &vars,
    );
    assert!(ok, "{out}{err}");
    assert_eq!(secret_rows(&pg), "1", "the App is sealed");
    let status = crate::service::ctl_over_the_socket(
        &env,
        &serde_json::json!({ "cmd": "github_app_status" }),
    );
    assert_eq!(status["data"]["configured"], true, "{status}");
    assert_eq!(local_github_files(&env), Vec::<PathBuf>::new());

    // A start finds the App in the vault and says so on its durable-state
    // line, without the row's name.
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
    let mut serve = Serve::start_bound(&env, "0.0.0.0", Some("info"), &vars);
    serve.wait_ready();
    let log = serve.log();
    let line = log
        .lines()
        .find(|l| l.contains("durable state: owner of postgres"))
        .unwrap_or_else(|| panic!("no durable state line:\n{log}"));
    assert!(line.ends_with("; GitHub App from vault"), "{line}");
    assert!(line.contains("(1 row)"), "{line}");

    let (ok, out, err) = run_vars(&env, &["github", "app", "remove"], &vars);
    assert!(ok, "{out}{err}");
    assert_eq!(secret_rows(&pg), "0", "the App row is deleted");
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );
    assert!(serve.alive());
}

/// In the pod, `connect github --personal` reports the share identity from
/// the vault it sealed in, not from "an App may be saved": with no App
/// saved, shares run on the instance identity and the hint to turn on
/// personal sharing is printed.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn connect_github_personal_in_the_pod_reports_the_checked_share_identity() {
    let Some(pg) = PgScratch::new("vtpodsi") else {
        return;
    };
    let github = PodGithub::start();
    let env = Env::new("vtpodsi").on_postgres(&pg);
    let vars = pod_vars(&github);
    let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut serve = Serve::start_bound(&env, "0.0.0.0", Some("info"), &vars);
    serve.wait_ready();

    let (ok, out, err) = run_vars(
        &env,
        &[
            "--json",
            "connect",
            "github",
            "--personal",
            "--token",
            "ghp_personal",
        ],
        &vars,
    );
    assert!(ok, "{out}{err}");
    let reply: Value = serde_json::from_str(out.trim()).unwrap_or_else(|_| panic!("{out}{err}"));
    assert_eq!(reply["token_store"], "vault", "{reply}");
    assert_eq!(
        reply["share_identity"], "instance",
        "no App is saved: {reply}"
    );
    let (ok, out, err) = run_vars(
        &env,
        &["connect", "github", "--personal", "--token", "ghp_personal"],
        &vars,
    );
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("Run: crystalline config set github.share_identity personal"),
        "{out}"
    );
    assert!(serve.alive());
}

/// In the pod with a key other than the one the rows were sealed with, a
/// sealing command is refused before it writes, naming both key ids, and
/// the rows are kept. Without a key it is refused naming the variable.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn sealing_beside_the_daemon_refuses_another_key() {
    let Some(pg) = PgScratch::new("vtpodkey") else {
        return;
    };
    let github = PodGithub::start();
    let env = Env::new("vtpodkey").on_postgres(&pg);
    let vars = pod_vars(&github);
    let mut with: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut serve = Serve::start_bound(&env, "0.0.0.0", None, &with);
    serve.wait_ready();
    seal_token_row(&pg, "github", "old-user");
    let other = "ff".repeat(32);
    with.push(("CRYSTALLINE_SECRET_KEY", &other));
    let (ok, out, err) = run_vars(&env, &["connect", "github"], &with);
    assert!(!ok, "{out}{err}");
    let mine = migrated_key_id_of(&pg);
    assert!(err.contains(&mine), "{err}");
    assert!(
        github.users_asked_with.lock().unwrap().is_empty(),
        "no sign-in"
    );
    with.pop();
    with.extend(NO_SECRET_KEY);
    let (ok, out, err) = run_vars(&env, &["connect", "github"], &with);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
    assert_eq!(secret_rows(&pg), "1", "the rows are kept");
    assert!(has_secret_row(&pg, "github"));
    assert!(serve.alive());
}

/// A Postgres configuration on a machine whose daemon serves another
/// database (a Turso one here): the secret commands never hand their change
/// to that daemon, which does not own the database. `users remove` claims
/// the free database and deletes the sealed token with the account, and the
/// daemon's own token file for that name stays; `connect github` seals
/// under its own claim, not beside that daemon.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn secret_commands_pass_by_a_daemon_of_another_database() {
    use crystalline_remote::{StoredToken, TokenIdentity, TokenStore};

    let Some(pg) = PgScratch::new("vtother") else {
        return;
    };
    let github = PodGithub::start();
    let env = Env::new("vtother");
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    let pg_vars = pg.env();
    let vars: Vec<(&str, &str)> = pg_vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let pod = pod_vars(&github);
    let with_github: Vec<(&str, &str)> = pg_vars
        .iter()
        .chain(pod.iter())
        .map(|(k, v)| (*k, v.as_str()))
        .collect();

    let (ok, out, err) = run_vars(
        &env,
        &["users", "add", "bob", "--mcp-token", "agent"],
        &vars,
    );
    assert!(ok, "{out}{err}");
    let row = personal_row("bob");
    seal_token_row(&pg, &row, "bob");
    // The Turso daemon's own token for a person of the same name.
    let origins = env.state_dir().join("origins");
    let local =
        TokenStore::file_fallback_for(&TokenIdentity::Personal("bob".to_string()), &origins)
            .unwrap();
    local
        .save(&StoredToken {
            access_token: "bob-local".to_string(),
            host: "github.com".to_string(),
            user: "bob".to_string(),
            created_at: crystalline_core::time::now(),
            refresh_token: None,
            expires_at: None,
            refresh_expires_at: None,
        })
        .unwrap();

    let (ok, out, err) = run_vars(&env, &["users", "remove", "bob"], &vars);
    assert!(ok, "{out}{err}");
    assert_eq!(accounts_named(&pg, "bob"), "0");
    assert!(!has_secret_row(&pg, &row), "the sealed token went too");
    assert!(
        local.load().unwrap().is_some(),
        "the Turso daemon's token is not touched"
    );
    assert_eq!(
        owner_command(&pg),
        "crystalline users remove|true",
        "the command claimed the database and gave it back"
    );

    let (ok, out, err) = run_vars(
        &env,
        &["connect", "github", "--token", "ghp_other"],
        &with_github,
    );
    assert!(ok, "{out}{err}");
    assert!(out.contains("(vault token store)"), "{out}");
    assert!(has_secret_row(&pg, &format!("github:{}", github.host())));
    assert_eq!(
        owner_command(&pg),
        "crystalline connect github|true",
        "sealed under the command's own claim"
    );
    let instance = TokenStore::file_fallback_for(&TokenIdentity::Instance, &origins).unwrap();
    assert!(instance.load().unwrap().is_none(), "nothing saved locally");
    assert!(serve.alive());
}

/// The owner row names a daemon of this state folder, but not the process
/// that answers on its control socket (the row of an earlier daemon,
/// before the next owner announced itself): no secret command routes to
/// the answering daemon. Without the key `users remove` is refused as with
/// no daemon; with it the claim meets the held lock and is refused. The
/// account and its sealed token both stay.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn a_daemon_the_owner_row_does_not_name_takes_no_secret_change() {
    let Some(pg) = PgScratch::new("vtstalepid") else {
        return;
    };
    let env = Env::new("vtstalepid").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    users_add(&env, "bob", "bob-password");
    let row = personal_row("bob");
    seal_token_row(&pg, &row, "bob");
    pg.run(&format!(
        "UPDATE {}.instance_owner SET pid = 1 WHERE id = 1",
        pg.schema
    ))
    .unwrap();

    let (ok, out, err) = run_vars(&env, &["users", "remove", "bob"], &NO_SECRET_KEY);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
    assert_eq!(accounts_named(&pg, "bob"), "1", "the account is kept");
    assert!(has_secret_row(&pg, &row), "the sealed token is kept");

    let (ok, out, err) = env.run_full(&["users", "remove", "bob"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("sealed in the database"), "{err}");
    assert_eq!(accounts_named(&pg, "bob"), "1", "the account is kept");
    assert!(has_secret_row(&pg, &row), "the sealed token is kept");
    assert!(serve.alive());
}

/// When who owns the database cannot be read (here no database answers at
/// all), a secret command is refused with a sentence that says so, not
/// with one about the key or another instance, and nothing is written.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn a_secret_command_whose_owner_cannot_be_read_is_refused_plainly() {
    let env = Env::new("vtunknown");
    let vars = [
        ("CRYSTALLINE_DATABASE_BACKEND", "postgres"),
        ("CRYSTALLINE_DATABASE_URL", "postgres://u@127.0.0.1:1/none"),
        ("CRYSTALLINE_SECRET_KEY", crate::service::TEST_SECRET_KEY),
        ("CRYSTALLINE_SECRET_KEY_FILE", ""),
    ];
    for args in [
        &["users", "remove", "bob"][..],
        &["connect", "github", "--token", "ghp_unknown"],
        &["connect", "https://kb.example", "--token"],
    ] {
        let (ok, out, err) = run_vars(&env, args, &vars);
        assert!(!ok, "{args:?}: {out}{err}");
        assert!(
            err.contains("could not check who owns the database"),
            "{args:?}: {err}"
        );
        assert!(
            err.contains("so nothing was changed; run the command again once the database answers"),
            "{args:?}: {err}"
        );
        assert!(!err.contains("ghp_unknown"), "the token is never repeated");
    }
    assert!(local_github_files(&env).is_empty());
}

/// A server sign-in on Postgres is made in the pod only: with the key on a
/// free database and no daemon, `connect <url>` and `disconnect <name>` are
/// refused before anyone signs in, with the `kubectl exec` example, and
/// nothing is written (no claim, no row, no `sources.json`).
#[cfg(feature = "postgres")]
#[test]
fn server_sign_in_away_from_the_pod_is_refused() {
    let Some(pg) = PgScratch::new("vtsrvaway") else {
        return;
    };
    let env = Env::new("vtsrvaway").on_postgres(&pg);
    let (ok, out, err) = env.run_full(&["connect", "https://kb.example", "--token"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("made in the pod"), "{err}");
    assert!(
        err.contains("kubectl exec <pod> -- crystalline connect https://kb.example --token"),
        "{err}"
    );
    let (ok, out, err) = env.run_full(&["disconnect", "acme"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("made in the pod"), "{err}");
    assert!(
        err.contains("kubectl exec <pod> -- crystalline disconnect acme"),
        "{err}"
    );
    assert_eq!(schema_objects(&pg), "0", "nothing was opened");
    assert!(
        !env.state_dir().join("remote").join("sources.json").exists(),
        "no sources.json"
    );
}

/// In the pod: `connect <url>` while the daemon owns the database seals the
/// server sign-in beside the daemon, tells it to reload its sources, and
/// the daemon serves the server's domain with the new credential. The
/// credential is not written to the state folder.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn connect_server_in_the_pod_seals_beside_the_daemon() {
    let Some(pg) = PgScratch::new("vtpodsrv") else {
        return;
    };
    let server = crate::remote_server::CliServer::start();
    let env = Env::new("vtpodsrv").on_postgres(&pg);
    let mut serve = Serve::start(&env, None);
    serve.wait_ready();
    assert_eq!(secret_rows(&pg), "0");

    let (ok, out, err) = run_with_stdin(
        &env,
        &["connect", &server.origin, "--name", "acme", "--token"],
        &format!("{}\n", server.token_for("keeper")),
    );
    assert!(ok, "{out}{err}");
    assert!(
        out.contains(&format!(
            "connected to {} as keeper (token); this machine calls it acme",
            server.origin
        )),
        "{out}"
    );
    let row = format!(
        "{}{}",
        crystalline_remote::server_token::SERVER_ACCOUNT_PREFIX,
        crystalline_remote::server_key(&server.origin)
    );
    assert!(has_secret_row(&pg, &row), "the sign-in is sealed");
    assert_eq!(secret_rows(&pg), "1");
    let remote = env.state_dir().join("remote");
    assert!(remote.join("sources.json").exists(), "the daemon's record");
    let credential = remote
        .join(crystalline_remote::server_folder(
            &crystalline_remote::server_key(&server.origin),
        ))
        .join("credential.json");
    assert!(
        !credential.exists(),
        "no credential file in the state folder"
    );

    // The daemon reloaded its sources and reads the server's domain with
    // the sealed credential: the command runs without the key, so it could
    // not open the credential itself.
    let (ok, out, err) = run_vars(
        &env,
        &["read", "vent-driver", "--domain", "open"],
        &NO_SECRET_KEY,
    );
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("the vent driver retries a write three times"),
        "{out}"
    );
    assert_eq!(
        owner_command(&pg),
        "|false",
        "the owner row stays the daemon's"
    );
    assert!(serve.alive());
}

/// The key id the rows of `pg` were sealed with.
#[cfg(feature = "postgres")]
fn migrated_key_id_of(pg: &PgScratch) -> String {
    pg.text(&format!(
        "SELECT min(key_id) FROM {}.instance_secret",
        pg.schema
    ))
}

/// Without a daemon and without the key, a daemonless origin command that
/// will reach GitHub is refused before its first write, naming the key.
#[cfg(feature = "postgres")]
#[test]
fn origin_update_without_key_refused_before_any_write() {
    let Some(pg) = PgScratch::new("vtorigin") else {
        return;
    };
    let env = Env::new("vtorigin").on_postgres(&pg);
    let (ok, out, err) = run_vars(&env, &["origin", "update"], &NO_SECRET_KEY);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("CRYSTALLINE_SECRET_KEY"), "{err}");
    assert_eq!(schema_objects(&pg), "0", "nothing was opened");
}

/// The id of the key every Postgres test process runs with.
#[cfg(feature = "postgres")]
pub(crate) fn test_key_id() -> String {
    use crystalline_remote::sealed::SealKey;

    use crate::service::TEST_SECRET_KEY;

    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&TEST_SECRET_KEY[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    SealKey::from_bytes(bytes.try_into().unwrap()).key_id()
}

/// Under the command's own claim (no daemon, the key set), `users disable`
/// of an account deletes its sealed token and disables it; disabling or
/// removing the last admin is refused before the token is touched, so the
/// row stays.
#[cfg(feature = "postgres")]
#[test]
fn users_disable_owned_and_the_last_admin_keep_the_vault_row() {
    let Some(pg) = PgScratch::new("vtadmin") else {
        return;
    };
    let env = Env::new("vtadmin").on_postgres(&pg);
    let (ok, out, err) = run_with_stdin(
        &env,
        &[
            "users",
            "add",
            "root",
            "--role",
            "admin",
            "--password-stdin",
        ],
        "root-password\n",
    );
    assert!(ok, "{out}{err}");
    users_add(&env, "bob", "bob-password");
    let (root, bob) = (personal_row("root"), personal_row("bob"));
    seal_token_row(&pg, &root, "root");
    seal_token_row(&pg, &bob, "bob");

    let (ok, out, err) = env.run_full(&["users", "disable", "bob"]);
    assert!(ok, "{out}{err}");
    assert!(!has_secret_row(&pg, &bob), "bob's token is deleted");
    let disabled = pg.text(&format!(
        "SELECT disabled::text FROM {}.users WHERE name = 'bob'",
        pg.schema
    ));
    assert_eq!(disabled, "1");

    for args in [
        &["users", "disable", "root"][..],
        &["users", "remove", "root"],
    ] {
        let (ok, out, err) = env.run_full(args);
        assert!(!ok, "{out}{err}");
        assert!(err.contains("the last admin ('root')"), "{err}");
        assert!(has_secret_row(&pg, &root), "the admin's token is kept");
        assert_eq!(accounts_named(&pg, "root"), "1");
    }
    let (ok, out, err) = env.run_full(&["users", "remove", "nobody"]);
    assert!(!ok, "{out}{err}");
    assert!(err.contains("no such user: 'nobody'"), "{err}");
}

/// Beside a daemon, a sealing command compares its key with the daemon's
/// even while no secret is sealed yet: another key is refused naming both
/// ids, and nothing is written.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn sealing_beside_a_daemon_with_another_key_is_refused_on_an_empty_table() {
    let Some(pg) = PgScratch::new("vtpodempty") else {
        return;
    };
    let github = PodGithub::start();
    let env = Env::new("vtpodempty").on_postgres(&pg);
    let vars = pod_vars(&github);
    let mut with: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut serve = Serve::start_bound(&env, "0.0.0.0", None, &with);
    serve.wait_ready();
    assert_eq!(secret_rows(&pg), "0");
    let other = "ee".repeat(32);
    with.push(("CRYSTALLINE_SECRET_KEY", &other));
    let (ok, out, err) = run_vars(&env, &["connect", "github"], &with);
    assert!(!ok, "{out}{err}");
    assert!(err.contains(&test_key_id()), "the daemon's key id: {err}");
    assert!(err.contains("CRYSTALLINE_SECRET_KEY here is key"), "{err}");
    // A key from the file variable is named as such.
    let file = env.state_dir().join("../other.key");
    std::fs::write(&file, &other).unwrap();
    let file = file.to_str().unwrap().to_string();
    with.pop();
    with.push(("CRYSTALLINE_SECRET_KEY", ""));
    with.push(("CRYSTALLINE_SECRET_KEY_FILE", &file));
    let (ok, out, err) = run_vars(&env, &["connect", "github"], &with);
    assert!(!ok, "{out}{err}");
    assert!(
        err.contains("CRYSTALLINE_SECRET_KEY_FILE here is key"),
        "{err}"
    );
    assert!(
        github.users_asked_with.lock().unwrap().is_empty(),
        "no sign-in"
    );
    assert_eq!(secret_rows(&pg), "0", "nothing was sealed");
    assert!(serve.alive());
}

/// On Postgres `status` and `doctor` read no local store: a GitHub token
/// file and a server sign-in file planted in the state folder are not
/// read; the sign-in shows as sealed in the database, and doctor's GitHub
/// section names the vault.
#[cfg(feature = "postgres")]
#[test]
fn reports_on_postgres_read_no_local_store() {
    use crystalline_remote::{ServerCredential, ServerCredentialStore, SourceRecord};
    use crystalline_remote::{StoredToken, TokenIdentity, TokenStore};

    let Some(pg) = PgScratch::new("vtlocal") else {
        return;
    };
    let env = Env::new("vtlocal").on_postgres(&pg);
    let origins = env.state_dir().join("origins");
    TokenStore::file_fallback_for(&TokenIdentity::Instance, &origins)
        .unwrap()
        .save(&StoredToken {
            access_token: "planted-secret".to_string(),
            host: "github.com".to_string(),
            user: "planted-user".to_string(),
            created_at: crystalline_core::time::now(),
            refresh_token: None,
            expires_at: None,
            refresh_expires_at: None,
        })
        .unwrap();
    let remote = env.state_dir().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let sources = serde_json::json!({ "v": 1, "sources": [{
        "url": "http://127.0.0.1:9",
        "name": "team",
        "account": "ada",
        "kind": "token",
        "connected_at": "2026-11-17T00:00:00Z",
    }]});
    std::fs::write(remote.join("sources.json"), sources.to_string()).unwrap();
    let record: SourceRecord = serde_json::from_value(sources["sources"][0].clone()).unwrap();
    ServerCredentialStore::file(&record.host_dir(&remote))
        .save(&ServerCredential::token(
            "planted-server-secret".to_string(),
            "http://127.0.0.1:9".to_string(),
            "ada".to_string(),
            crystalline_core::time::now(),
        ))
        .unwrap();
    let github = [("CRYSTALLINE_GITHUB_ENABLED", "true")];

    let (ok, out, err) = run_vars(&env, &["--json", "status"], &github);
    assert!(ok, "{out}{err}");
    assert!(out.contains(r#""credential_store":"vault""#), "{out}");
    assert!(!out.contains(r#""credential_store":"file""#), "{out}");
    let (_, out, err) = run_vars(&env, &["--json", "doctor"], &github);
    let doctor: Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}{err}"));
    assert_eq!(doctor["github"]["token_store"], "vault", "{doctor}");
    assert!(doctor["github"]["user"].is_null(), "{doctor}");
    let text = format!("{out}{err}");
    assert!(!text.contains("planted-user"), "{text}");
    assert!(!text.contains(r#""credential_store":"file""#), "{text}");
    // The planted files are named in one warning, by their names alone.
    let warnings = doctor["durable_state"]["warnings"].to_string();
    assert!(
        warnings.contains("not read while Postgres is configured")
            && warnings.contains("github-token.json")
            && warnings.contains("credential.json"),
        "{doctor}"
    );
    let (ok, out, err) = run_vars(&env, &["status"], &github);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("Warning: the credential files ")
            && out.contains("github-token.json")
            && !out.contains("planted"),
        "{out}"
    );
    // The default share identity depends on an App that would be sealed in
    // the database: not checked, and said so.
    let (ok, out, err) = run_vars(&env, &["--json", "config", "show"], &github);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains(r#""key":"github.share_identity","source":"default","value":"sealed in the database (not checked)""#),
        "{out}"
    );
}

/// `--config` decides the backend for the whole process: with the default
/// configuration on Postgres, a command run with `--config` naming a Turso
/// file keeps its credentials where Turso keeps them, and nothing is
/// refused for a vault that is not there.
#[cfg(all(unix, feature = "postgres"))]
#[test]
fn a_turso_config_flag_wins_over_a_postgres_default() {
    let github = PodGithub::start();
    let env = Env::new("vtflag");
    let default = env.config_path();
    std::fs::create_dir_all(default.parent().unwrap()).unwrap();
    std::fs::write(
        &default,
        "database:\n  backend: postgres\n  url: postgres://u@127.0.0.1:1/none\n",
    )
    .unwrap();
    let turso = default.parent().unwrap().join("turso.yaml");
    std::fs::write(&turso, format!("github:\n  api_url: {}\n", github.url)).unwrap();
    let tokens = default.parent().unwrap().join("tokens");
    let tokens_var = tokens.to_str().unwrap().to_string();
    let (ok, out, err) = run_vars(
        &env,
        &[
            "connect",
            "github",
            "--token",
            "ghp_flag",
            "--config",
            turso.to_str().unwrap(),
        ],
        &[("CRYSTALLINE_TEST_TOKEN_STORE_DIR", &tokens_var)],
    );
    assert!(ok, "{out}{err}");
    assert!(!err.contains("vault"), "{err}");
    let saved: Vec<_> = std::fs::read_dir(&tokens)
        .map(|dir| dir.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(!saved.is_empty(), "the token is in the Turso file store");
}

/// The main-level look at the configuration is quiet: a warning the
/// command's own load gives about the file is said once, not twice.
#[test]
fn a_config_warning_is_said_once_per_command() {
    let env = Env::new("vtwarn");
    let (_, out, err) = run_vars(
        &env,
        &["users", "list"],
        &[("CRYSTALLINE_SERVICE_PUBLIC_URL", "http://0.0.0.0:7411")],
    );
    let text = format!("{out}{err}");
    let warning = "must be an address a browser can open";
    assert_eq!(text.matches(warning).count(), 1, "{text}");
}
