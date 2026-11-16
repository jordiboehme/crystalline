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
        Serve::start_with(env, rust_log, &[])
    }

    /// [`Serve::start`] with `extra` variables for this child only.
    fn start_with(env: &Env, rust_log: Option<&str>, extra: &[(&str, &str)]) -> Serve {
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
        for (key, value) in extra {
            cmd.env(key, value);
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
    let log = serve.log();
    assert!(
        !log.contains("releasing the database"),
        "Turso has no database to release:\n{log}"
    );
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
fn instance_id(env: &Env) -> String {
    std::fs::read_to_string(env.state_dir().join("instance-id"))
        .expect("the instance id")
        .trim()
        .to_string()
}

/// The owner row of `pg`'s schema as `instance_id|released`, where
/// `released` is `t` or `f`; empty when there is no row.
fn owner_row(pg: &PgScratch) -> String {
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
    active_env.setup_domain("notes");
    let mut active = Serve::start_with(
        &active_env,
        None,
        &[("CRYSTALLINE_TEST_FENCE_EXIT_HOLD_MS", FENCE_EXIT_HOLD_MS)],
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
    // `setup_domain` put the domain beside the state folder's base.
    let domain_dir = active_env
        .state_dir()
        .ancestors()
        .nth(2)
        .unwrap()
        .join("kb-notes");
    assert!(domain_dir.is_dir(), "{}", domain_dir.display());
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
    let standby_env = Env::new("hohs").on_postgres(&pg);
    standby_env.setup_domain("notes");
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
    let mut cmd = Command::new(bin());
    env.apply(&mut cmd);
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
/// `config set` touches no database in this release and still works.
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
    let (ok, out, err) = env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(ok, "{out}{err}");

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

    let (ok, out, err) = env.run_full(&["status"]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        owner_command(&pg),
        "crystalline reindex|true",
        "status claimed the database"
    );
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
