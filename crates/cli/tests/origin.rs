//! Smoke tests for the GitHub-origin CLI verbs, against a temp config and a
//! temp index, no daemon involved (the in-process path).
//!
//! Every scenario here is reachable without a network call: the CLI's own
//! flag validation (`--origin` combined with `--virtual` or `--no-sync`, or
//! `--branch` without `--origin`, or a malformed `--origin` value) runs
//! before anything talks to `crystalline-service`, and `github.enabled`
//! being off refuses before an engine method ever tries to build a GitHub
//! provider. The successful connect/update/status paths against a real (or
//! mocked) origin are covered at the engine level by
//! `crates/service/tests/origin.rs`, which injects a mock provider; there is
//! no HTTP-mocking harness in this crate to exercise them here, and
//! `connect github` needs a live GitHub connection to test end to end, so it
//! is not covered by an automated test in this crate (noted as a gap; its
//! auth building blocks are covered by `crates/remote`'s own
//! `github_auth.rs`/`github_client.rs` tests).

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

// --- domain add --origin: flag validation (no network) -----------------------

#[test]
fn domain_add_origin_and_virtual_conflict() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["domain", "add", "brand", "--origin", "acme/brand-knowledge"])
        .args(["--virtual", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--virtual"));
}

#[test]
fn domain_add_origin_and_no_sync_conflict() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["domain", "add", "brand", "--origin", "acme/brand-knowledge"])
        .args(["--no-sync", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--no-sync"));
}

#[test]
fn domain_add_branch_without_origin_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let dir = work.path().join("kb");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), "# Manifest").unwrap();
    bin()
        .args(["domain", "add", "eng"])
        .arg(&dir)
        .args(["--branch", "main", "--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--origin"));
}

#[test]
fn domain_add_origin_rejects_a_malformed_spec() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    bin()
        .args(["domain", "add", "brand", "--origin", "not-a-repo"])
        .args(["--config"])
        .arg(&config)
        .assert()
        .failure()
        .stderr(predicates::str::contains("owner/repo"));
}

// --- gating: github.enabled, reached through the real CLI plumbing -----------

#[test]
fn domain_add_origin_refuses_when_github_is_not_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");
    bin()
        .args(["domain", "add", "brand", "--origin", "acme/brand-knowledge"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));
}

#[test]
fn origin_update_and_status_refuse_when_github_is_not_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["origin", "update", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));

    bin()
        .args(["origin", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));
}

#[test]
fn origin_update_and_status_succeed_with_no_team_domains_once_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["config", "set", "github.enabled", "true", "--config"])
        .arg(&config)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "origin", "update", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["domains"].as_array().unwrap().len(), 0);
    assert_eq!(data["errors"].as_array().unwrap().len(), 0);

    let out = bin()
        .args(["--json", "origin", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let data: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["connection"]["connected"], false);
    assert_eq!(data["domains"].as_array().unwrap().len(), 0);

    // The human render mentions no domains and the disconnected state,
    // without panicking on the empty arrays.
    let human = bin()
        .args(["origin", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(human.status.success());
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains("not connected"), "{human}");
    assert!(human.contains("No team domains"), "{human}");
}

// --- origin share, withdraw, resolve: flag validation and gating ------------

#[test]
fn origin_resolve_requires_exactly_one_of_keep_or_content_file() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    // Neither given.
    bin()
        .args(["origin", "resolve", "brand", "notes/a.md", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("--keep"))
        .stderr(predicates::str::contains("--content-file"));
}

#[test]
fn origin_resolve_rejects_both_keep_and_content_file() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");
    let content_file = work.path().join("merged.md");
    std::fs::write(&content_file, "merged content").unwrap();

    bin()
        .args(["origin", "resolve", "brand", "notes/a.md"])
        .args(["--keep", "mine", "--content-file"])
        .arg(&content_file)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("exactly one"));
}

#[test]
fn origin_share_withdraw_and_resolve_refuse_when_github_is_not_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["origin", "share", "brand", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));

    bin()
        .args(["origin", "withdraw", "brand", "--proposal", "1", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));

    bin()
        .args(["origin", "resolve", "brand", "notes/a.md", "--keep", "mine"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("github.enabled"));
}

#[test]
fn origin_share_withdraw_and_resolve_reach_the_engine_once_enabled() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["config", "set", "github.enabled", "true", "--config"])
        .arg(&config)
        .assert()
        .success();

    // No such domain is registered, so each verb reaches the engine's real
    // domain-lookup error rather than failing at CLI flag parsing.
    bin()
        .args(["origin", "share", "brand", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not registered"));

    bin()
        .args(["origin", "withdraw", "brand", "--proposal", "1", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not registered"));

    // --proposal is optional now: omitting it means "the single open one",
    // which still reaches the engine rather than tripping flag parsing.
    bin()
        .args(["origin", "withdraw", "brand", "--revert", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not registered"));

    bin()
        .args(["origin", "resolve", "brand", "notes/a.md", "--keep", "mine"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not registered"));
}

#[test]
fn origin_share_help_names_the_amend_flag() {
    bin()
        .args(["origin", "share", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--proposal"))
        .stdout(predicates::str::contains("Amend this open proposal"));
}

#[test]
fn origin_discard_is_gone_and_withdraw_help_names_its_flags() {
    bin()
        .args(["origin", "discard", "brand", "--proposal", "1"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("unrecognized subcommand"));

    bin()
        .args(["origin", "withdraw", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--proposal"))
        .stdout(predicates::str::contains("--revert"));
}

// --- chain rendering, against a stand-in daemon ------------------------------

/// The stacked-chain render paths, driven end to end through the real binary
/// against a stand-in daemon: an owner record naming this alive test process
/// plus a ctl socket that answers one command with canned JSON and hands the
/// request back to the test. That is what makes both halves observable at
/// once - what the CLI sends (`origin share --proposal 6` really carries the
/// number) and what it renders from what a daemon answers.
///
/// Unix-only: a filesystem socket the test binds itself is the mechanism, and
/// a short `/tmp` base keeps the path under the `sockaddr_un` limit. Neither
/// `--config` nor `--db` may be passed here, since either override sends the
/// verb down the in-process path instead of to the socket.
#[cfg(unix)]
mod chain {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use assert_cmd::Command;
    use serde_json::{Value, json};

    /// A stand-in daemon answering exactly one ctl command.
    struct Daemon {
        dir: PathBuf,
        requests: Receiver<Value>,
    }

    impl Daemon {
        /// Bind the socket, write the owner record and serve one command with
        /// `data` as its payload.
        fn answering(tag: &str, data: Value) -> Daemon {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = PathBuf::from("/tmp").join(format!("cq-chain-{tag}-{nanos}"));
            let state = dir.join("state/crystalline");
            std::fs::create_dir_all(&state).unwrap();
            std::fs::create_dir_all(dir.join("config")).unwrap();
            std::fs::create_dir_all(dir.join("cache")).unwrap();

            let sock = state.join("service.sock");
            let listener = UnixListener::bind(&sock).unwrap();
            let record = json!({
                "pid": std::process::id(),
                "socket_path": sock.display().to_string(),
                // This binary's own version: an older one would be displaced
                // rather than attached to.
                "version": env!("CARGO_PKG_VERSION"),
                "started_at": "2026-08-27T00:00:00Z",
            });
            std::fs::write(state.join("service.json"), record.to_string()).unwrap();

            let (tx, requests) = channel();
            std::thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                // The mode line ("ctl") comes first, then the command.
                let mut mode = String::new();
                let mut line = String::new();
                if reader.read_line(&mut mode).is_err() || reader.read_line(&mut line).is_err() {
                    return;
                }
                if let Ok(request) = serde_json::from_str::<Value>(line.trim()) {
                    let _ = tx.send(request);
                }
                let mut write = stream;
                let envelope = json!({ "v": 1, "ok": true, "data": data });
                let _ = writeln!(write, "{envelope}");
                let _ = write.flush();
            });
            Daemon { dir, requests }
        }

        /// Run the real binary against this daemon and return its stdout.
        fn run(&self, args: &[&str]) -> String {
            let mut cmd = Command::cargo_bin("crystalline").unwrap();
            cmd.env("HOME", &self.dir)
                .env("XDG_CONFIG_HOME", self.dir.join("config"))
                .env("XDG_STATE_HOME", self.dir.join("state"))
                .env("XDG_CACHE_HOME", self.dir.join("cache"))
                .env("CRYSTALLINE_SERVICE_HTTP", "false")
                .args(args);
            let out = cmd.output().unwrap();
            assert!(out.status.success(), "{out:?}");
            String::from_utf8(out.stdout).unwrap()
        }

        /// The single ctl command the CLI sent.
        fn request(&self) -> Value {
            self.requests
                .recv_timeout(Duration::from_secs(30))
                .expect("the CLI sent a ctl command")
        }
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn share_with_a_proposal_number_amends_that_layer_and_names_the_chain() {
        let daemon = Daemon::answering(
            "amend",
            json!({
                "outcome": "updated",
                "proposal": {
                    "number": 6,
                    "url": "https://github.test/acme/brand/pull/6",
                    "summary": "3 engrams refined",
                    "added": [], "updated": ["notes/a.md"], "deleted": [],
                    "skipped_large": [],
                    "stack_number": 42,
                    "stack_position": [2, 3],
                },
            }),
        );
        let out = daemon.run(&["origin", "share", "brand", "--proposal", "6"]);
        let request = daemon.request();
        assert_eq!(request["cmd"], "origin_share");
        assert_eq!(request["proposal"], 6);
        assert!(out.contains("Updated proposal #6"), "{out}");
        assert!(
            out.contains("proposal #6, layer 2 of 3 on stack #42"),
            "{out}"
        );
    }

    #[test]
    fn a_plain_share_stacks_a_new_layer_and_says_the_link_is_pending() {
        let daemon = Daemon::answering(
            "stack",
            json!({
                "outcome": "proposed",
                "url": "https://github.test/acme/brand/pull/8",
                "number": 8,
                "branch": "crystalline/brand-8",
                "summary": "2 engrams added",
                "added": ["notes/b.md"], "updated": [], "deleted": [],
                "skipped_large": [],
                // The chain exists; the call that groups it on the forge did
                // not land, so there is no stack number to name.
                "stack_number": Value::Null,
                "stack_position": [2, 2],
            }),
        );
        let out = daemon.run(&["origin", "share", "brand"]);
        let request = daemon.request();
        assert!(request["proposal"].is_null(), "{request}");
        assert!(
            out.contains("proposal #8, layer 2 of 2 (stack link pending)"),
            "{out}"
        );
        assert!(!out.contains("stack #"), "{out}");
    }

    #[test]
    fn a_lone_open_proposal_is_rendered_without_any_layer_framing() {
        let daemon = Daemon::answering(
            "lone",
            json!({
                "outcome": "proposed",
                "url": "https://github.test/acme/brand/pull/8",
                "number": 8,
                "branch": "crystalline/brand-8",
                "summary": "2 engrams added",
                "added": ["notes/b.md"], "updated": [], "deleted": [],
                "skipped_large": [],
                "stack_number": 42,
                "stack_position": [1, 1],
            }),
        );
        let out = daemon.run(&["origin", "share", "brand"]);
        assert!(out.contains("Opened proposal:"), "{out}");
        assert!(!out.contains("layer"), "{out}");
        assert!(!out.contains("stack"), "{out}");
    }

    #[test]
    fn refreshed_folder_indexes_get_one_line_and_stay_out_of_the_counts() {
        let daemon = Daemon::answering(
            "indexes",
            json!({
                "outcome": "proposed",
                "url": "https://github.test/acme/brand/pull/9",
                "number": 9,
                "branch": "crystalline/brand-9",
                "summary": "Shares 1 new engram.",
                "added": ["notes/b.md", "index.md"],
                "updated": ["notes/index.md"],
                "deleted": [],
                "skipped_large": [],
                "stack_number": Value::Null,
                "stack_position": Value::Null,
            }),
        );
        let out = daemon.run(&["origin", "share", "brand"]);
        // The counts are about the engram; the listings that rode along with
        // it say so once, underneath, and never inflate the numbers a reader
        // recognizes their own work in.
        assert!(out.contains("1 added, 0 updated, 0 deleted"), "{out}");
        assert!(out.contains("also refreshes 2 folder indexes"), "{out}");
    }

    #[test]
    fn a_share_with_no_refreshed_indexes_says_nothing_about_them() {
        let daemon = Daemon::answering(
            "plain",
            json!({
                "outcome": "proposed",
                "url": "https://github.test/acme/brand/pull/9",
                "number": 9,
                "branch": "crystalline/brand-9",
                "summary": "Shares 1 new engram.",
                "added": ["notes/b.md"], "updated": [], "deleted": [],
                "skipped_large": [],
                "stack_number": Value::Null,
                "stack_position": Value::Null,
            }),
        );
        let out = daemon.run(&["origin", "share", "brand"]);
        assert!(out.contains("1 added, 0 updated, 0 deleted"), "{out}");
        assert!(!out.contains("folder index"), "{out}");
    }

    #[test]
    fn withdraw_names_the_repaired_chain_and_what_it_could_not_restore() {
        let daemon = Daemon::answering(
            "repaired",
            json!({
                "number": 7,
                "closed": true,
                "status": "withdrawn",
                "restored": ["notes/a.md"],
                "deleted": [],
                "skipped_diverged": [],
                "skipped_reverts": ["notes/d.md"],
                "repaired": true,
                "restacked": 43,
            }),
        );
        let out = daemon.run(&["origin", "withdraw", "brand", "--proposal", "7", "--revert"]);
        assert!(out.contains("Withdrew proposal #7"), "{out}");
        assert!(out.contains("stack repaired; now stack #43"), "{out}");
        assert!(
            out.contains("could not restore (no reachable copy): notes/d.md"),
            "{out}"
        );
    }

    #[test]
    fn withdrawing_down_to_one_survivor_says_the_stack_dissolved() {
        let daemon = Daemon::answering(
            "dissolved",
            json!({
                "number": 7,
                "closed": true,
                "status": "withdrawn",
                "restored": [], "deleted": [], "skipped_diverged": [],
                "skipped_reverts": [],
                "repaired": true,
                "restacked": Value::Null,
            }),
        );
        let out = daemon.run(&["origin", "withdraw", "brand", "--proposal", "7"]);
        assert!(out.contains("stack dissolved"), "{out}");
        assert!(!out.contains("now stack"), "{out}");
    }

    /// The status payload for a domain: `open` open proposals in chain order
    /// plus whatever chain state the test is about.
    fn status_payload(open: Vec<Value>, wedged: Vec<u64>, repair: bool, link: bool) -> Value {
        json!({
            "connection": { "connected": true, "user": "octocat", "token_store": "keychain" },
            "domains": [{
                "domain": "brand",
                "repo": "acme/brand-knowledge",
                "branch": "main",
                "base_commit": "abc123",
                "behind": false,
                "local_changes": 2,
                "skipped_large": [],
                "open_proposals": open,
                "declined_proposals": [{
                    "number": 7, "title": "Declined work",
                    "url": "https://github.test/acme/brand/pull/7",
                }],
                "conflicts": [],
                "last_checked": Value::Null,
                "probe_error": Value::Null,
                "stack_number": 42,
                "stack_wedged": wedged,
                "repair_pending": repair,
                "stack_link_pending": link,
            }],
            "errors": [],
        })
    }

    fn open_proposal(number: u64, title: &str) -> Value {
        json!({
            "number": number,
            "title": title,
            "url": format!("https://github.test/acme/brand/pull/{number}"),
            "status": "open",
        })
    }

    #[test]
    fn status_renders_the_chain_bottom_up_with_its_debts() {
        let daemon = Daemon::answering(
            "status-chain",
            status_payload(
                vec![
                    open_proposal(3, "Bottom layer"),
                    open_proposal(8, "Middle layer"),
                    open_proposal(9, "Top layer"),
                ],
                vec![7],
                true,
                true,
            ),
        );
        let out = daemon.run(&["origin", "status"]);
        assert!(out.contains("stack #42: 3 layers"), "{out}");
        assert!(out.contains("layer 1: open proposal #3"), "{out}");
        assert!(out.contains("layer 3: open proposal #9"), "{out}");
        assert!(
            out.contains("stack wedged by #7 - withdraw it or share to repair"),
            "{out}"
        );
        assert!(
            out.contains("repair pending - the next share or withdraw finishes it"),
            "{out}"
        );
        assert!(
            out.contains("stack link pending - a share or status with connection retries it"),
            "{out}"
        );
    }

    #[test]
    fn status_names_no_stack_for_a_chain_the_forge_holds_none_of() {
        let mut payload = status_payload(
            vec![
                open_proposal(3, "Bottom layer"),
                open_proposal(8, "Top layer"),
            ],
            vec![],
            false,
            true,
        );
        payload["domains"][0]["stack_number"] = Value::Null;
        let daemon = Daemon::answering("status-unlinked", payload);
        let out = daemon.run(&["origin", "status"]);
        assert!(out.contains("layer 2: open proposal #8"), "{out}");
        assert!(!out.contains("stack #"), "{out}");
    }

    #[test]
    fn status_leaves_a_single_open_proposal_unframed_and_quiet() {
        let daemon = Daemon::answering(
            "status-lone",
            status_payload(vec![open_proposal(3, "Only work")], vec![], false, false),
        );
        let out = daemon.run(&["origin", "status"]);
        assert!(out.contains("open proposal #3: Only work"), "{out}");
        assert!(!out.contains("layer"), "{out}");
        assert!(!out.contains("wedged"), "{out}");
        assert!(!out.contains("pending"), "{out}");
    }
}
