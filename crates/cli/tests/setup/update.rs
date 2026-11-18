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
/// answers with `status` and `body`, anything else is a 404. It counts the
/// API requests. Plain HTTP/1.1, one request per connection.
#[cfg(debug_assertions)]
struct FakeGitHub {
    base: String,
    api_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(debug_assertions)]
impl FakeGitHub {
    fn start(status: u16, body: String) -> FakeGitHub {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = api_requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line == "\r\n" => break,
                        Ok(_) => {}
                    }
                }
                let api =
                    request_line.starts_with("GET /repos/jordiboehme/crystalline/releases/latest ");
                let (status, body) = if api {
                    count.fetch_add(1, Ordering::SeqCst);
                    (status, body.as_str())
                } else {
                    (404, "")
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        FakeGitHub { base, api_requests }
    }

    fn api_requests(&self) -> usize {
        self.api_requests.load(std::sync::atomic::Ordering::SeqCst)
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
    cmd.args(["update", "check"]).args(args).output().unwrap()
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
    assert!(stdout.contains("update now: nothing: this is the newest stable release\n"));
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
            "Crystalline 99.0.0 is available (this is {CURRENT}).\nPublished: 2026-01-01T00:00:00Z\nInstall method: "
        )),
        "{stdout}"
    );
    // A test build is no official build, so `update now` would refuse.
    assert!(
        stdout.contains("\nupdate now: nothing: not an official release build\n"),
        "{stdout}"
    );
    assert!(
        stdout.ends_with(
            "\nRelease notes: https://github.com/jordiboehme/crystalline/releases/tag/v99.0.0\n"
        ),
        "{stdout}"
    );
    // A second run asks GitHub again, whatever the cache holds.
    let again = update_check(home.path(), &fake, &[], &[]);
    assert_eq!(again.status.code(), Some(10));
    assert_eq!(fake.api_requests(), 2);
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
