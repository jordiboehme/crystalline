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
struct Serve {
    child: Child,
    addr: String,
    log: PathBuf,
}

impl Serve {
    fn start(env: &Env, rust_log: Option<&str>) -> Serve {
        let addr = format!("127.0.0.1:{}", free_port());
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
        let child = cmd
            .args(["serve", "--http", &addr])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        Serve { child, addr, log }
    }

    fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    fn log(&self) -> String {
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
    fn wait_role(&mut self, role: &str) {
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
    fn wait_ready(&mut self) {
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
    fn wait_exit(&mut self, within: Duration) -> Option<i32> {
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

    fn alive(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    fn terminate(&mut self) {
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
}
