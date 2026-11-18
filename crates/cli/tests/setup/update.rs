//! `crystalline update verify-sums`: the publish job's proof that the
//! release key pair works. Hidden, and it trusts only the compiled-in keys.

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../core/tests/fixtures/update")
        .join(name)
}

#[test]
fn verify_sums_refuses_a_test_signature() {
    let sums = fixture("v0.0.0-SHA256SUMS");
    let sig = fixture("v0.0.0-SHA256SUMS.test-key.minisig");
    assert!(sums.is_file() && sig.is_file(), "the fixtures exist");
    let out = crate::common::crystalline()
        .args(["update", "verify-sums", "--tag", "v0.0.0"])
        .arg(&sums)
        .arg(&sig)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr.trim(),
        "SHA256SUMS is signed by a key this build does not trust"
    );
    assert!(out.stdout.is_empty(), "nothing on stdout");
}

#[test]
fn verify_sums_is_hidden() {
    for args in [&["--help"][..], &["update", "--help"][..]] {
        let out = crate::common::crystalline().args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}");
        let help = String::from_utf8_lossy(&out.stdout);
        assert!(!help.contains("verify-sums"), "{args:?} names it: {help}");
    }
}

/// `update verify-sums` with the minisign CLI test key trusted through the
/// debug-only seam.
#[cfg(debug_assertions)]
fn verify_sums_with_the_test_key(tag: &str) -> std::process::Output {
    crate::common::crystalline()
        .env("CRYSTALLINE_TEST_UPDATE_KEY", fixture("cli-test.pub"))
        .args(["update", "verify-sums", "--tag", tag])
        .arg(fixture("v0.0.0-SHA256SUMS"))
        .arg(fixture("v0.0.0-SHA256SUMS.cli-test-key.minisig"))
        .output()
        .unwrap()
}

#[cfg(debug_assertions)]
#[test]
fn verify_sums_accepts_a_signature_by_a_trusted_key() {
    let out = verify_sums_with_the_test_key("v0.0.0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "verified SHA256SUMS for v0.0.0\n"
    );
    assert!(stderr.is_empty(), "nothing on stderr: {stderr}");
}

#[cfg(debug_assertions)]
#[test]
fn verify_sums_refuses_a_signature_for_another_tag() {
    let out = verify_sums_with_the_test_key("v0.0.1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr.trim(),
        "the signature over SHA256SUMS is for \"tag:v0.0.0\", not for tag v0.0.1"
    );
    assert!(out.stdout.is_empty(), "nothing on stdout");
}

/// A release build has no key seam: the environment cannot add a trusted key.
#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_ignores_the_test_key() {
    let out = crate::common::crystalline()
        .env("CRYSTALLINE_TEST_UPDATE_KEY", fixture("cli-test.pub"))
        .args(["update", "verify-sums", "--tag", "v0.0.0"])
        .arg(fixture("v0.0.0-SHA256SUMS"))
        .arg(fixture("v0.0.0-SHA256SUMS.cli-test-key.minisig"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "SHA256SUMS is signed by a key this build does not trust"
    );
}

/// S11: the key seam is read in exactly one place, an item compiled only
/// into debug builds. It lives in core (`update::trusted_keys`), which both
/// this CLI and the service's update client use, so no crate reads it twice.
#[test]
fn the_test_key_seam_is_debug_only() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let name = "\"CRYSTALLINE_TEST_UPDATE_KEY\"";
    let mut reads = Vec::new();
    for krate in ["cli", "core", "service"] {
        // Every .rs file under src, subfolders included.
        let mut files = Vec::new();
        let mut dirs = vec![crates.join(krate).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (at, line) in lines.iter().enumerate() {
                if !line.contains(name) {
                    continue;
                }
                reads.push(format!("{krate}: {}", path.display()));
                // The read sits in a function whose attribute is the cfg.
                let item = lines[..at]
                    .iter()
                    .rposition(|l| l.starts_with("fn ") || l.starts_with("pub"))
                    .expect("the read sits in an item");
                assert_eq!(
                    lines[item - 1].trim(),
                    "#[cfg(debug_assertions)]",
                    "{}:{}: the read of {name} is not debug-only",
                    path.display(),
                    at + 1
                );
            }
        }
    }
    assert_eq!(
        reads.len(),
        1,
        "the seam is read in exactly one place: {reads:?}"
    );
    assert!(reads[0].starts_with("core: "), "{reads:?}");
}

/// A fake GitHub on 127.0.0.1 for `update check`: the API's latest release
/// answers with `status` and `body` and the ETag [`ETAG`], or with 304 to a
/// request that sends that ETag back; anything else is a 404. It counts the
/// API requests and the 304 answers, and every request it saw. Plain
/// HTTP/1.1, one request per connection.
#[cfg(debug_assertions)]
struct FakeGitHub {
    base: String,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    api_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    not_modified: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

/// The ETag the fake answers with.
#[cfg(debug_assertions)]
const ETAG: &str = "\"e1\"";

#[cfg(debug_assertions)]
impl FakeGitHub {
    fn start(status: u16, body: String) -> FakeGitHub {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_all = requests.clone();
        let api_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let not_modified = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = api_requests.clone();
        let count_304 = not_modified.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                count_all.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut etag_sent = false;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line == "\r\n" => break,
                        Ok(_) => {
                            let lower = line.to_ascii_lowercase();
                            if lower.starts_with("if-none-match:") && line.contains(ETAG) {
                                etag_sent = true;
                            }
                        }
                    }
                }
                let api =
                    request_line.starts_with("GET /repos/jordiboehme/crystalline/releases/latest ");
                let (status, body) = if !api {
                    (404, "")
                } else if etag_sent {
                    count.fetch_add(1, Ordering::SeqCst);
                    count_304.fetch_add(1, Ordering::SeqCst);
                    (304, "")
                } else {
                    count.fetch_add(1, Ordering::SeqCst);
                    (status, body.as_str())
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\netag: {ETAG}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        FakeGitHub {
            base,
            requests,
            api_requests,
            not_modified,
        }
    }

    fn requests(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn api_requests(&self) -> usize {
        self.api_requests.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn not_modified(&self) -> usize {
        self.not_modified.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(debug_assertions)]
fn release(tag: &str, prerelease: bool, published_at: &str) -> String {
    serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": prerelease,
        "published_at": published_at,
        "assets": []
    })
    .to_string()
}

/// An old publish time: the age gate is open.
#[cfg(debug_assertions)]
const LONG_AGO: &str = "2026-01-01T00:00:00Z";

/// `crystalline update check <args>` in its own home against `fake`.
#[cfg(debug_assertions)]
fn update_check(
    home: &std::path::Path,
    fake: &FakeGitHub,
    args: &[&str],
    env: &[(&str, &str)],
) -> std::process::Output {
    update(home, fake, "check", args, env)
}

/// `crystalline update <verb> <args>` in its own home against `fake`.
#[cfg(debug_assertions)]
fn update(
    home: &std::path::Path,
    fake: &FakeGitHub,
    verb: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = crate::common::crystalline_std();
    for (name, value) in crate::common::isolation_env(home) {
        cmd.env(name, value);
    }
    cmd.env("CRYSTALLINE_TEST_UPDATE_API", &fake.base)
        .env("CRYSTALLINE_TEST_UPDATE_DOWNLOAD", &fake.base)
        .env_remove("CRYSTALLINE_UPDATE_MIN_AGE_HOURS")
        .env_remove("CRYSTALLINE_UPDATE_CHECK")
        .env_remove("CRYSTALLINE_CONFIG");
    for proxy in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(proxy);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.args(["update", verb]).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(debug_assertions)]
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[cfg(debug_assertions)]
#[test]
fn check_up_to_date_exits_0() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release(&format!("v{CURRENT}"), false, LONG_AGO));
    let out = update_check(home.path(), &fake, &[], &[]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.starts_with(&format!(
            "Crystalline {CURRENT} is up to date (newest stable release: {CURRENT}).\n"
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains("update now: does nothing (this is the newest stable release)\n"),
        "{stdout}"
    );
    assert_eq!(fake.api_requests(), 1);
    // The check is kept where the daemon's daily check keeps it.
    let cache = crate::common::isolated_state_dir(home.path()).join("update-check.json");
    assert!(cache.is_file(), "the check wrote {}", cache.display());
}

#[cfg(debug_assertions)]
#[test]
fn check_newer_exits_10_and_names_the_action_and_notes_url() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v99.0.0", false, LONG_AGO));
    let out = update_check(home.path(), &fake, &[], &[]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(10), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.starts_with(&format!(
            "Crystalline 99.0.0 is available (this is {CURRENT}).\nPublished: 2026-01-01T00:00:00Z ("
        )),
        "{stdout}"
    );
    assert!(stdout.contains(" days ago)\nInstall method: "), "{stdout}");
    // A test build is no official build, so `update now` would refuse.
    assert!(
        stdout.contains("\nupdate now: does nothing (not an official release build)\n"),
        "{stdout}"
    );
    assert!(
        stdout.ends_with(
            "\nRelease notes: https://github.com/jordiboehme/crystalline/releases/tag/v99.0.0\n"
        ),
        "{stdout}"
    );
    // A second run asks GitHub again, whatever the cache holds, with the
    // ETag it kept: GitHub's "not modified" keeps the release and moves the
    // time of the check.
    let cache_path = crate::common::isolated_state_dir(home.path()).join("update-check.json");
    let checked_at = |path: &std::path::Path| {
        let cache: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        cache["checked_at"].as_str().unwrap().to_string()
    };
    let first_checked = checked_at(&cache_path);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let again = update_check(home.path(), &fake, &[], &[]);
    assert_eq!(again.status.code(), Some(10), "{}", text(&again.stderr));
    // The same plan (the age on the publish line may tick over a day).
    let without_age = |out: &str| {
        out.lines()
            .filter(|line| !line.starts_with("Published: "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(without_age(&text(&again.stdout)), without_age(&stdout));
    assert_eq!(fake.api_requests(), 2);
    assert_eq!(fake.not_modified(), 1, "the ETag was sent back");
    assert_ne!(checked_at(&cache_path), first_checked);
}

#[cfg(debug_assertions)]
#[test]
fn check_gated_exits_0_and_says_when() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v99.0.0", false, "2099-01-01T00:00:00Z"));
    let out = update_check(home.path(), &fake, &[], &[]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.starts_with(&format!(
            "Crystalline 99.0.0 is out (this is {CURRENT}), but too new to offer: it is offered from 2099-01-02T00:00:00Z. --ignore-age offers it now.\n"
        )),
        "{stdout}"
    );
    // The gate follows update.min_age_hours.
    let out = update_check(
        home.path(),
        &fake,
        &[],
        &[("CRYSTALLINE_UPDATE_MIN_AGE_HOURS", "720")],
    );
    assert!(
        text(&out.stdout).contains("offered from 2099-01-31T00:00:00Z"),
        "{}",
        text(&out.stdout)
    );
}

#[cfg(debug_assertions)]
#[test]
fn ignore_age_skips_the_gate() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v99.0.0", false, "2099-01-01T00:00:00Z"));
    let out = update_check(home.path(), &fake, &["--ignore-age"], &[]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(10), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.starts_with("Crystalline 99.0.0 is available"),
        "{stdout}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn json_shape() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v99.0.0", false, "2099-01-01T00:00:00Z"));
    let out = update_check(home.path(), &fake, &["--json"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let keys: Vec<&str> = plan
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut keys = keys;
    keys.sort();
    assert_eq!(
        keys,
        [
            "action",
            "current",
            "gate",
            "method",
            "newest",
            "notes_url",
            "published_at"
        ]
    );
    assert_eq!(plan["current"], CURRENT);
    assert_eq!(plan["newest"], "99.0.0");
    assert_eq!(plan["published_at"], "2099-01-01T00:00:00Z");
    assert_eq!(
        plan["gate"],
        serde_json::json!({"opens_at": "2099-01-02T00:00:00Z"})
    );
    assert_eq!(
        plan["action"],
        serde_json::json!({"refuse": "not_official"})
    );
    assert!(plan["method"].is_string());
    assert_eq!(
        plan["notes_url"],
        "https://github.com/jordiboehme/crystalline/releases/tag/v99.0.0"
    );

    let out = update_check(home.path(), &fake, &["--json", "--ignore-age"], &[]);
    assert_eq!(out.status.code(), Some(10), "exit 10 with --json too");
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["gate"], "open");
}

#[cfg(debug_assertions)]
#[test]
fn check_never_offers_an_older_or_pre_release() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v0.0.1", false, LONG_AGO));
    let out = update_check(home.path(), &fake, &[], &[]);
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.starts_with(&format!(
            "Crystalline {CURRENT} is up to date (newest stable release: 0.0.1).\n"
        )),
        "{stdout}"
    );

    // A pre-release, or a tag that is not exactly v<x.y.z>, is never taken:
    // the check fails in a fixed sentence and the tag is shown nowhere.
    for (tag, prerelease) in [
        ("v99.0.0-rc.1", false),
        ("v99.0.0", true),
        ("v99.0.0<b>", false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let fake = FakeGitHub::start(200, release(tag, prerelease, LONG_AGO));
        let out = update_check(home.path(), &fake, &[], &[]);
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert_eq!(out.status.code(), Some(1), "{tag}: {stdout}{stderr}");
        assert!(stdout.is_empty(), "{tag}: {stdout}");
        assert!(
            stderr.contains(
                "update check failed: the newest release on GitHub is not a stable release"
            ),
            "{tag}: {stderr}"
        );
        assert!(!stderr.contains("99.0.0"), "{tag}: {stderr}");
    }
}

#[cfg(debug_assertions)]
#[test]
fn a_failed_check_exits_1() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(502, "down".to_string());
    let out = update_check(home.path(), &fake, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(
        text(&out.stderr).contains("update check failed: GitHub answered with status 502"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn update_is_listed_and_check_documents_exit_10() {
    let out = crate::common::crystalline().arg("--help").output().unwrap();
    assert!(
        text(&out.stdout).contains("  update "),
        "{}",
        text(&out.stdout)
    );
    let out = crate::common::crystalline()
        .args(["update", "check", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = text(&out.stdout);
    assert!(help.contains("10 an update is available"), "{help}");
    assert!(help.contains("--ignore-age"), "{help}");
}

/// A test build is no official build, so `update now` refuses before it
/// asks GitHub anything or writes a file, with `--yes` too.
#[cfg(debug_assertions)]
#[test]
fn an_unofficial_build_refuses_before_any_download() {
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v99.0.0", false, LONG_AGO));
    for args in [&[][..], &["--yes", "--ignore-age"][..]] {
        let out = update(home.path(), &fake, "now", args, &[]);
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty(), "{args:?}: {}", text(&out.stdout));
        assert!(
            stderr.contains("update now does nothing: not an official release build"),
            "{args:?}: {stderr}"
        );
    }
    assert_eq!(fake.requests(), 0, "the fake saw no request");
    // No lock and no download folder anywhere in the home.
    let mut dirs = vec![home.path().to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_ne!(path.file_name().unwrap(), "updates", "{}", path.display());
                dirs.push(path);
            }
        }
    }
}

#[test]
fn now_documents_its_exit_codes_and_flags() {
    let out = crate::common::crystalline()
        .args(["update", "now", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = text(&out.stdout);
    for needle in [
        "--yes",
        "--ignore-age",
        "10 an update is available but nothing was done",
        "brew upgrade jordiboehme/tap/crystalline",
    ] {
        assert!(help.contains(needle), "{needle}: {help}");
    }
}

/// A Postgres schema of this test's own, with the configuration document
/// table and `body` as its document; dropped at the end. `None`, with a
/// note, when `CRYSTALLINE_TEST_POSTGRES_URL` is unset.
#[cfg(all(debug_assertions, feature = "postgres"))]
struct PgDocument {
    url: String,
    schema: String,
}

#[cfg(all(debug_assertions, feature = "postgres"))]
impl PgDocument {
    fn new(body: &str) -> Option<PgDocument> {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            eprintln!("note: skipping the postgres leg (CRYSTALLINE_TEST_POSTGRES_URL is unset)");
            return None;
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let doc = PgDocument {
            url,
            schema: format!("updcheck_{nanos}"),
        };
        // The table as the migrations make it (crates/index postgres
        // migrations, `instance_config`).
        doc.run(&format!(
            "CREATE SCHEMA {s}; \
             CREATE TABLE {s}.instance_config (\
                 id SMALLINT PRIMARY KEY CHECK (id = 1), body JSONB NOT NULL, \
                 updated_at TIMESTAMPTZ NOT NULL, updated_by TEXT NOT NULL); \
             INSERT INTO {s}.instance_config (id, body, updated_at, updated_by) \
             VALUES (1, '{body}', now(), 'test')",
            s = doc.schema
        ))
        .expect(
            "CRYSTALLINE_TEST_POSTGRES_URL is set, but its database could not take a test schema",
        );
        Some(doc)
    }

    fn run(&self, sql: &str) -> Result<(), sqlx::Error> {
        use sqlx::Connection;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut conn = sqlx::PgConnection::connect(&self.url).await?;
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
                    .execute(&mut conn)
                    .await?;
                Ok(())
            })
    }

    /// The variables that point a process at this schema.
    fn env(&self) -> [(&'static str, &str); 3] {
        [
            ("CRYSTALLINE_DATABASE_BACKEND", "postgres"),
            ("CRYSTALLINE_DATABASE_URL", &self.url),
            ("CRYSTALLINE_DATABASE_SCHEMA", &self.schema),
        ]
    }
}

#[cfg(all(debug_assertions, feature = "postgres"))]
impl Drop for PgDocument {
    fn drop(&mut self) {
        let _ = self.run(&format!("DROP SCHEMA {} CASCADE", self.schema));
    }
}

/// On Postgres the settings live in the configuration document: the gate
/// follows the `update.min_age_hours` saved there, as the daemon's does.
#[cfg(all(debug_assertions, feature = "postgres"))]
#[test]
fn the_gate_follows_the_postgres_settings_document() {
    let Some(doc) = PgDocument::new(r#"{"update": {"min_age_hours": 720}}"#) else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let fake = FakeGitHub::start(200, release("v99.0.0", false, "2099-01-01T00:00:00Z"));
    let out = update_check(home.path(), &fake, &[], &doc.env());
    let stdout = text(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}{}", text(&out.stderr));
    assert!(
        stdout.contains("offered from 2099-01-31T00:00:00Z"),
        "{stdout}{}",
        text(&out.stderr)
    );
}
