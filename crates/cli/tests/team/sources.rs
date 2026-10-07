//! `crystalline connect <url>`, `crystalline disconnect <name|url>`, a local
//! rename of a mounted domain, and what `status` and `doctor` say about
//! every source.

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

use crate::common::{crystalline, isolate};
use crate::remote_server::CliServer;

fn bin(home: &Path) -> Command {
    let mut cmd = crystalline();
    isolate(&mut cmd, home);
    cmd
}

fn connect(server: &CliServer, home: &Path, name: &str) -> String {
    let out = bin(home)
        .args(["connect", &server.origin, "--name", name, "--token"])
        .write_stdin(format!("{}\n", server.token_for("keeper")))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn status_json(home: &Path) -> Value {
    let out = bin(home).args(["status", "--json"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn mounts(source: &Value) -> Vec<(String, String)> {
    source["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["local"].as_str().unwrap().to_string(),
                m["remote"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn connect_with_a_pasted_token_lists_the_source_in_status() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let said = connect(&server, home.path(), "acme");
    assert!(
        said.contains(&format!(
            "connected to {} as keeper (token); this machine calls it acme",
            server.origin
        )),
        "{said}"
    );
    let status = status_json(home.path());
    let acme = &status["sources"][0];
    assert_eq!(acme["name"], "acme");
    assert_eq!(acme["url"], server.origin.as_str());
    assert_eq!(acme["account"], "keeper");
    assert_eq!(acme["kind"], "token");
    assert_eq!(acme["reachable"], true, "{status}");
    assert_eq!(mounts(acme), vec![("open".to_string(), "open".to_string())]);
    assert!(
        status.get("indexed").is_some(),
        "the local report is still there: {status}"
    );
}

/// An account with no password and one token is enough for an agent: the
/// token opens `/api/v1/ctl` (connect asks it for the server's domains) under
/// the account's own name.
#[test]
fn a_token_only_account_connects_and_lists_its_mounts() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let out = bin(home.path())
        .args(["connect", &server.origin, "--name", "acme", "--token"])
        .write_stdin(format!("{}\n", server.agent_token("agent-build")))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status = status_json(home.path());
    let acme = &status["sources"][0];
    assert_eq!(acme["account"], "agent-build", "{status}");
    assert_eq!(acme["reachable"], true, "{status}");
    assert_eq!(mounts(acme), vec![("open".to_string(), "open".to_string())]);
}

/// Review focus 2: the second server's `open` gets the suffix, said at once,
/// and the first keeps its name.
#[test]
fn connecting_a_second_server_announces_the_collision_name_and_keeps_the_first() {
    let first = CliServer::start();
    let second = CliServer::start_with(&["specs"]);
    let home = tempfile::tempdir().unwrap();
    connect(&first, home.path(), "acme");
    let said = connect(&second, home.path(), "beta");
    assert!(
        said.contains("beta: the domain 'open' is called 'open-beta' on this machine, because 'open' was already taken"),
        "{said}"
    );
    let status = status_json(home.path());
    assert_eq!(
        mounts(&status["sources"][0]),
        vec![("open".to_string(), "open".to_string())]
    );
    let beta = mounts(&status["sources"][1]);
    assert!(
        beta.contains(&("open-beta".to_string(), "open".to_string())),
        "{beta:?}"
    );
    assert!(
        beta.contains(&("specs".to_string(), "specs".to_string())),
        "{beta:?}"
    );
}

/// A path the rules refuse is named with its reason, which lists the
/// characters a path may use; a word that is no address is never repeated.
#[test]
fn a_refused_path_says_which_characters_a_path_may_use() {
    let home = tempfile::tempdir().unwrap();
    let said = |args: &[&str]| {
        let out = bin(home.path()).args(args).output().unwrap();
        assert!(!out.status.success(), "{args:?}");
        String::from_utf8_lossy(&out.stderr).to_string()
    };
    let path = said(&["connect", "https://kb.example/Crystalline"]);
    assert!(path.contains("that is not a server address"), "{path}");
    assert!(
        path.contains("may use only lower-case letters, digits, '.', '_' and '-'"),
        "{path}"
    );
    let word = said(&["connect", "not-an-address"]);
    assert!(word.contains("that is not a server address"), "{word}");
    assert!(!word.contains("not-an-address"), "{word}");
}

/// Ruling F19: a token anywhere on the command line is refused in the same
/// sentence, and its text is never repeated on either stream.
#[test]
fn a_token_on_the_command_line_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let forms: [&[&str]; 11] = [
        // Re-review R1 and R2: a `github` word that is not the subcommand, a
        // third word, and a token as the --name value.
        &[
            "connect",
            "https://kb.example",
            "--name",
            "github",
            "--token=cmt_SECRET7",
        ],
        &[
            "connect",
            "--name",
            "github",
            "--token=cmt_SECRET8",
            "https://kb.example",
        ],
        &["connect", "https://kb.example", "extra", "cmt_SECRET9"],
        &["connect", "https://kb.example", "--name", "cmt_SECRET10"],
        &["connect", "https://kb.example", "--token", "cmt_SECRET0"],
        // The `connect github --token <value>` habit.
        &["connect", "--token", "cmt_SECRET1", "https://kb.example"],
        &["connect", "--token", "cmt_SECRET2"],
        &["connect", "cmt_SECRET3", "--token"],
        &["connect", "coa_SECRET4"],
        &["connect", "https://kb.example", "--token=cmt_SECRET5"],
        &["connect", "--token=cor_SECRET6", "https://kb.example"],
    ];
    for form in forms {
        let out = bin(home.path()).args(form).output().unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "{form:?}: {said}");
        assert!(
            said.contains("refusing a token on the command line"),
            "{form:?}: {said}"
        );
        assert!(
            !said.contains("SECRET"),
            "{form:?} repeats the token: {said}"
        );
    }
}

/// Re-review R5: `github` after a flag is read as the server address, so
/// it gets a sentence saying where it goes, and no token is repeated.
#[test]
fn github_after_a_flag_says_to_put_it_first() {
    let home = tempfile::tempdir().unwrap();
    let forms: [&[&str]; 4] = [
        &["connect", "--json", "github", "--token=cmt_SECRET1"],
        &["connect", "--json", "github", "--token=ghp_SECRET2"],
        &["connect", "--db", "/tmp/x", "github", "--token=cmt_SECRET3"],
        &["connect", "--json", "github"],
    ];
    for form in forms {
        let out = bin(home.path()).args(form).output().unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "{form:?}: {said}");
        assert!(
            said.contains("put github right after connect, before any flag"),
            "{form:?}: {said}"
        );
        assert!(
            !said.contains("SECRET"),
            "{form:?} repeats the token: {said}"
        );
    }
}

/// Re-review R6: the scan looks only at the `connect` command itself.
#[test]
fn a_connect_word_inside_another_command_is_not_scanned() {
    let home = tempfile::tempdir().unwrap();
    let out = bin(home.path())
        .args(["search", "connect", "cmt_like_words"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("refusing a token"), "{stderr}");
}

/// Review M2: a word after the URL that is no token is named as unexpected,
/// with the flag that was probably meant.
#[test]
fn an_extra_word_after_the_url_points_to_the_name_flag() {
    let home = tempfile::tempdir().unwrap();
    let out = bin(home.path())
        .args(["connect", "https://kb.example", "acme"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("use --name <name>"), "{stderr}");
    assert!(!stderr.contains("refusing a token"), "{stderr}");
}

/// A home whose own configuration registers `team-platform`, tracking the
/// repository the server's `platform` tracks: the same domain.
fn home_with_a_team_copy(home: &Path) -> std::path::PathBuf {
    let dir = home.join("team-platform");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: platform\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# platform\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("local-note.md"),
        "---\ntype: engram\ntitle: Local\npermalink: local-note\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Local\n\n- [fact] not shared yet\n",
    )
    .unwrap();
    let mut cfg = crystalline_core::config::GlobalConfig::default();
    let mut entry = crystalline_core::config::DomainEntry::file(&dir);
    entry.origin = Some(crystalline_core::config::OriginConfig {
        repo: "acme/platform".into(),
        path: None,
        branch: None,
        poll_secs: None,
    });
    // Re-review R4: an alias of the copy is refused in the same words.
    entry.aliases.push("old-platform".into());
    cfg.domains.insert("team-platform".into(), entry);
    crystalline_core::config::save_yaml(&home.join("config.yaml"), &cfg).unwrap();
    // An origin state with nothing in its base: every file is unshared work.
    crystalline_remote::state::OriginState::new("acme/platform", "main")
        .save(
            &crate::common::isolated_state_dir(home)
                .join("origins")
                .join("team-platform"),
        )
        .unwrap();
    dir
}

/// [`bin`] reading the home's own configuration file.
fn bin_cfg(home: &Path) -> Command {
    let mut cmd = bin(home);
    cmd.env("CRYSTALLINE_CONFIG", home.join("config.yaml"));
    cmd
}

/// Decision D19 and review I1: connect warns about the unshared work of the
/// local copy it hides, naming the copy; and the hidden copy can be neither
/// removed nor renamed, with or without a daemon, in the words status uses.
#[test]
fn a_hidden_copy_is_warned_about_and_cannot_be_removed_or_renamed() {
    let server = CliServer::start_full(&[], &[("platform", "acme/platform")]);
    let home = tempfile::tempdir().unwrap();
    let dir = home_with_a_team_copy(home.path());
    let out = bin_cfg(home.path())
        .args(["connect", &server.origin, "--name", "acme", "--token"])
        .write_stdin(format!("{}\n", server.token_for("keeper")))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("warning: your local copy of 'team-platform' holds"),
        "{stderr}"
    );
    let hidden = "the local domain 'team-platform' is hidden while acme is connected; disconnect acme to use it again";
    let refused = |args: &[&str]| {
        let out = bin_cfg(home.path()).args(args).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "{args:?} went through: {stderr}");
        assert!(stderr.contains(hidden), "{args:?}: {stderr}");
    };
    // The mount took the copy's name, so a remove of that name meets the
    // hidden copy.
    let out = bin_cfg(home.path())
        .args(["status", "--json"])
        .output()
        .unwrap();
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        mounts(&status["sources"][0])
            .contains(&("team-platform".to_string(), "platform".to_string())),
        "{status}"
    );
    assert_eq!(
        status["sources"][0]["hidden_local"],
        serde_json::json!(["team-platform"]),
        "{status}"
    );
    refused(&["domain", "remove", "team-platform"]);
    refused(&["domain", "remove", "team-platform", "--purge"]);
    // Once the mount has a name of its own, the copy is reached by its name
    // alone, and still refused.
    bin_cfg(home.path())
        .args([
            "domain",
            "rename",
            "team-platform",
            "acme-platform",
            "--local",
        ])
        .assert()
        .success();
    refused(&["domain", "remove", "team-platform", "--purge"]);
    refused(&["domain", "rename", "team-platform", "other"]);
    refused(&["domain", "rename", "team-platform", "other", "--local"]);
    refused(&["domain", "rename", "old-platform", "other"]);
    refused(&["domain", "remove", "old-platform", "--purge"]);
    assert!(dir.join("local-note.md").is_file(), "the files stay");

    // The same through a running daemon.
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut serve = crate::common::crystalline_std();
    for (name, value) in crate::common::isolation_env(home.path()) {
        serve.env(name, value);
    }
    serve.env("CRYSTALLINE_CONFIG", home.path().join("config.yaml"));
    let mut daemon = serve
        .args(["serve", "--http", "off"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !bin_cfg(home.path())
        .args(["ctl", "status", "--json"])
        .output()
        .unwrap()
        .status
        .success()
    {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "daemon not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    refused(&["domain", "remove", "team-platform", "--purge"]);
    refused(&["domain", "rename", "team-platform", "other"]);
    let _ = bin_cfg(home.path()).args(["ctl", "shutdown"]).output();
    let _ = daemon.wait();
    assert!(dir.join("local-note.md").is_file(), "the files stay");
}

/// Review M1: the note about a sign-in the server could not end is printed.
#[test]
fn disconnect_says_when_the_server_could_not_end_the_sign_in() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    // Make it a browser sign-in whose revocation the server must answer.
    let remote = crate::common::isolated_state_dir(home.path()).join("remote");
    let sources_file = remote.join("sources.json");
    let mut sources: Value =
        serde_json::from_slice(&std::fs::read(&sources_file).unwrap()).unwrap();
    sources["sources"][0]["kind"] = "oauth".into();
    sources["sources"][0]["revocation_endpoint"] =
        format!("{}/api/v1/oauth/revoke", server.origin).into();
    std::fs::write(&sources_file, serde_json::to_vec(&sources).unwrap()).unwrap();
    let credential = walk(&remote)
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == "credential.json"))
        .expect("the credential is a file under the test switch");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&credential).unwrap()).unwrap();
    saved["kind"] = "oauth".into();
    saved["refresh_token"] = "cor_test".into();
    std::fs::write(&credential, serde_json::to_vec(&saved).unwrap()).unwrap();
    server.stop();
    let out = bin(home.path())
        .args(["disconnect", "acme"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("note: acme ("), "{stderr}");
    assert!(
        stderr.contains("could not be reached to end the sign-in there"),
        "{stderr}"
    );
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Review M1 and M3: doctor names a server that is down as a problem, and
/// checks no source under a --config override.
#[test]
fn doctor_counts_a_server_that_is_down_and_skips_sources_under_an_override() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    server.stop();
    let out = bin(home.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!out.status.success(), "a problem fails doctor: {report}");
    let problem = report["sources"][0]["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("cannot be reached right now"), "{report}");
    let human = bin(home.path()).args(["doctor"]).output().unwrap();
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("PROBLEM: acme ("), "{text}");

    let other = home.path().join("other.yaml");
    crystalline_core::config::save_yaml(&other, &crystalline_core::config::GlobalConfig::default())
        .unwrap();
    let out = bin(home.path())
        .args(["doctor", "--json", "--config", other.to_str().unwrap()])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["sources"], serde_json::json!([]), "{report}");
}

/// Spec A1 (ruling F16): a server without browser sign-in asks for a token
/// instead of stopping, and says why.
#[test]
fn a_server_without_browser_sign_in_asks_for_a_token() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let out = bin(home.path())
        .args(["connect", &server.origin, "--name", "acme"])
        .write_stdin(format!("{}\n", server.token_for("keeper")))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("does not offer browser sign-in"),
        "{stderr}"
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("as keeper (token); this machine calls it acme"),
        "{said}"
    );
}

#[test]
fn plain_http_off_this_machine_is_refused() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .args(["connect", "http://kb.example", "--token"])
        .write_stdin("cmt_x\n")
        .assert()
        .failure()
        .stderr(predicates::str::contains("refusing plain http"));
}

#[test]
fn disconnect_removes_one_source_by_name_or_url() {
    let first = CliServer::start();
    let second = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&first, home.path(), "acme");
    connect(&second, home.path(), "beta");
    bin(home.path())
        .args(["disconnect", "acme"])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "disconnected from acme ({})",
            first.origin
        )));
    let status = status_json(home.path());
    let names: Vec<&str> = status["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["beta"]);
    bin(home.path())
        .args(["disconnect", &second.origin])
        .assert()
        .success();
    bin(home.path())
        .args(["disconnect", "acme"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "no connected server is called acme",
        ));
}

/// Spec A3: `domain rename --local` works on a mounted domain and is kept.
#[test]
fn a_local_rename_of_a_mounted_domain_is_kept_and_a_shared_rename_is_refused() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    bin(home.path())
        .args(["domain", "rename", "open", "acme-open", "--local"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "'open' from acme is called 'acme-open' on this machine now",
        ));
    let status = status_json(home.path());
    assert_eq!(
        mounts(&status["sources"][0]),
        vec![("acme-open".to_string(), "open".to_string())]
    );
    bin(home.path())
        .args(["domain", "rename", "acme-open", "other"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("'acme-open' comes from acme; only its name on this machine can change here: add --local"));
}

#[test]
fn doctor_lists_every_source() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    let out = bin(home.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["sources"][0]["name"], "acme");
    assert_eq!(report["sources"][0]["reachable"], true, "{report}");
    assert!(report["sources"][0]["problem"].is_null(), "{report}");
}

#[test]
fn status_with_a_server_down_names_it_and_keeps_the_others() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    server.stop();
    let status = status_json(home.path());
    assert_eq!(status["sources"][0]["reachable"], false, "{status}");
    assert!(
        status["sources"][0]["error"]
            .as_str()
            .unwrap()
            .contains("cannot be reached right now"),
        "{status}"
    );
}

/// With a daemon running: `connect` tells it at once (the `sources_reload`
/// sender), and `status` shows the daemon's rows with what this run asked
/// each server added.
#[test]
fn a_running_daemon_learns_of_a_connect_at_once_and_status_merges_its_rows() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let mut serve = crate::common::crystalline_std();
    for (name, value) in crate::common::isolation_env(home.path()) {
        serve.env(name, value);
    }
    // No HTTP: the default port is shared by every daemon on this machine.
    let mut daemon = serve
        .args(["serve", "--http", "off"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon_status = || -> Option<Value> {
        let out = bin(home.path())
            .args(["ctl", "status", "--json"])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| serde_json::from_slice(&out.stdout).unwrap())
    };
    let start = Instant::now();
    while daemon_status().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "daemon not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    connect(&server, home.path(), "acme");
    // Right after connect, before the daemon's own look at the file (5 s).
    let told = daemon_status().unwrap();
    let names: Vec<&str> = told["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["acme"], "{told}");

    let status = status_json(home.path());
    assert!(status.get("pid").is_some(), "the daemon answered: {status}");
    let acme = &status["sources"][0];
    assert_eq!(acme["name"], "acme");
    assert_eq!(acme["reachable"], true, "{status}");
    assert!(
        acme.get("failure").is_some(),
        "the daemon's key stays: {status}"
    );
    assert_eq!(mounts(acme), vec![("open".to_string(), "open".to_string())]);

    let _ = bin(home.path()).args(["ctl", "shutdown"]).output();
    let _ = daemon.wait();
}

/// Review M7: a mounted name with a configuration that does not load gets
/// the configuration's own error.
#[test]
fn a_local_rename_of_a_mount_says_when_the_configuration_does_not_load() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    std::fs::write(home.path().join("config.yaml"), "domains: [not a mapping\n").unwrap();
    let out = bin_cfg(home.path())
        .args(["domain", "rename", "open", "acme-open", "--local"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("'open' comes from a connected server, and its name on this machine cannot change while this machine's configuration does not load"),
        "{stderr}"
    );
}
