//! End-to-end tests for `crystalline agents`, which writes the accounts store
//! directly like `crystalline users`, in an isolated home with no daemon.

use assert_cmd::Command;

use crate::common::isolate;

/// Run `crystalline <args>` in the isolated home: success, stdout, stderr.
fn run(home: &std::path::Path, args: &[&str], stdin: Option<&str>) -> (bool, String, String) {
    run_with_mcp_auth(home, args, stdin, true)
}

/// [`run`] with `auth.mcp` as `mcp_auth` says: agents need it on.
fn run_with_mcp_auth(
    home: &std::path::Path,
    args: &[&str],
    stdin: Option<&str>,
    mcp_auth: bool,
) -> (bool, String, String) {
    let mut cmd: Command = crate::common::crystalline();
    isolate(&mut cmd, home);
    cmd.env(
        "CRYSTALLINE_AUTH_MCP",
        if mcp_auth { "true" } else { "false" },
    );
    cmd.args(args);
    if let Some(input) = stdin {
        cmd.write_stdin(input.to_string());
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The whole life of an agent from the command line: add prints the token
/// once, list shows it without the token, a second token is issued and a
/// third refused, one is revoked, and remove takes the agent with its
/// tokens. Accounts and agents share one namespace, both ways, and an agent
/// is never listed as an account.
#[test]
fn agents_add_list_token_revoke_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();

    let (ok, out, err) = run(
        home,
        &[
            "--json",
            "agents",
            "add",
            "Nightly",
            "--display",
            "Nightly runner",
            "--loops",
            "evolve",
            "--expires",
            "30d",
        ],
        None,
    );
    assert!(ok, "{err}");
    let added: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(added["agent"]["name"], "nightly");
    assert_eq!(added["agent"]["loops"], "evolve");
    assert_eq!(added["agent"]["created_by"], "cli");
    let token = added["token"]["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cag_"), "{out}");
    let first_id = added["token"]["id"].as_i64().unwrap().to_string();

    let (ok, out, err) = run(home, &["agents", "list"], None);
    assert!(ok, "{err}");
    assert!(
        out.contains("nightly (Nightly runner): the evolve loop"),
        "{out}"
    );
    assert!(out.contains("live until"), "{out}");
    assert!(!out.contains(&token), "the token is never shown again");

    let (ok, out, err) = run(
        home,
        &["agents", "token", "nightly", "--expires", "7"],
        None,
    );
    assert!(ok, "{err}");
    assert!(out.contains("cag_"), "{out}");
    let (ok, _, err) = run(home, &["agents", "token", "nightly"], None);
    assert!(!ok);
    assert!(err.contains("already holds 2 live tokens"), "{err}");
    let (ok, _, err) = run(
        home,
        &["agents", "token", "nightly", "--expires", "400d"],
        None,
    );
    assert!(!ok);
    assert!(err.contains("--expires takes a number of days"), "{err}");

    let (ok, out, err) = run(home, &["agents", "revoke", "nightly", &first_id], None);
    assert!(ok, "{err}");
    assert!(out.contains("Revoked token"), "{out}");
    let (ok, _, err) = run(home, &["agents", "revoke", "nightly", &first_id], None);
    assert!(!ok, "a second revoke finds nothing");
    assert!(err.contains("no unrevoked token"), "{err}");
    let (_, out, _) = run(home, &["agents", "list"], None);
    assert!(out.contains("revoked"), "{out}");

    // One namespace, both ways.
    let (ok, _, err) = run(
        home,
        &["users", "add", "nightly", "--password-stdin"],
        Some("pw12345678\n"),
    );
    assert!(!ok);
    assert!(err.contains("belongs to an agent"), "{err}");
    let (ok, _, err) = run(
        home,
        &["users", "add", "ada", "--role", "admin", "--password-stdin"],
        Some("pw12345678\n"),
    );
    assert!(ok, "{err}");
    let (ok, _, err) = run(home, &["agents", "add", "ADA"], None);
    assert!(!ok);
    assert!(err.contains("belongs to an account"), "{err}");
    let (_, out, _) = run(home, &["users", "list"], None);
    assert!(
        !out.contains("nightly"),
        "an agent is not an account: {out}"
    );

    let (ok, out, err) = run(home, &["agents", "remove", "nightly"], None);
    assert!(ok, "{err}");
    assert!(out.contains("Removed agent 'nightly'"), "{out}");
    let (ok, _, err) = run(home, &["agents", "remove", "nightly"], None);
    assert!(!ok);
    assert!(err.contains("no such agent"), "{err}");
    let (_, out, _) = run(home, &["agents", "list"], None);
    assert!(out.contains("No agents"), "{out}");
}

/// With `auth.mcp` off `agents add` makes no agent and says why: an agent
/// token would open nothing on this instance.
#[test]
fn agents_add_is_refused_while_auth_mcp_is_off() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let (ok, _, err) = run_with_mcp_auth(home, &["agents", "add", "nightly"], None, false);
    assert!(!ok);
    assert!(
        err.contains("Agents need auth.mcp on")
            && err.contains("crystalline config set auth.mcp true"),
        "{err}"
    );
    let (ok, out, err) = run(home, &["agents", "list"], None);
    assert!(ok, "{err}");
    assert!(!out.contains("nightly"), "{out}");
}

/// Doctor warns when this instance holds agents and `auth.mcp` is off, and
/// says nothing about them once it is on.
#[test]
fn doctor_warns_about_agents_while_auth_mcp_is_off() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let (ok, _, err) = run(home, &["agents", "add", "nightly"], None);
    assert!(ok, "{err}");
    let (_, out, err) = run_with_mcp_auth(home, &["--json", "doctor"], None, false);
    let report: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}{err}"));
    assert_eq!(
        report["agents_need_auth_mcp"],
        serde_json::json!(["nightly"]),
        "{out}"
    );
    let (_, out, _) = run_with_mcp_auth(home, &["doctor"], None, false);
    assert!(
        out.contains("1 agent of the loops (nightly) and auth.mcp is off"),
        "{out}"
    );
    let (_, out, err) = run(home, &["--json", "doctor"], None);
    let report: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}{err}"));
    assert!(report.get("agents_need_auth_mcp").is_none(), "{out}");
}
