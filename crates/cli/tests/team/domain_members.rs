//! End-to-end tests for `crystalline domain members`, `domain visibility` and
//! `domain transfer` - who may reach a private domain, administered from the
//! machine that holds the files.
//!
//! Every child runs with its own `HOME` and XDG base directories (`isolate`),
//! so the accounts database these commands write lands in a temp state
//! directory rather than in the developer's own `web-auth.db`, and the domain
//! registry lives in a config file inside the same temp home.
//!
//! The property under test throughout: the CLI is the machine operator and is
//! refused by no web role. It administers a domain it was never invited to,
//! because whoever can run it already holds the files.

use assert_cmd::Command;

use crate::common::isolate;

fn bin() -> Command {
    crate::common::crystalline()
}

/// A workspace: an isolated home holding a registered file domain `eng` and
/// two accounts, `ada` and `bob`. Returns the home and the config path every
/// domain-addressed command below is pointed at.
struct Fixture {
    home: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.yaml");
        let domain_dir = home.path().join("kb");
        let fx = Fixture { home, config };
        fx.run(
            &["domain", "init"],
            &[domain_dir.to_str().unwrap(), "--name", "eng"],
        );
        // `--no-sync` keeps the index out of this suite entirely: nothing here
        // reads an engram, and registration is all the membership commands
        // need in order to resolve the name.
        fx.domain(&["add", "eng", domain_dir.to_str().unwrap(), "--no-sync"]);
        for (name, role) in [("ada", "editor"), ("bob", "viewer")] {
            fx.stdin(
                &["users", "add", name, "--role", role, "--password-stdin"],
                "s3cret\n",
            );
        }
        fx
    }

    /// Run a command that must succeed, returning stdout.
    fn run(&self, head: &[&str], tail: &[&str]) -> String {
        let mut cmd = bin();
        isolate(&mut cmd, self.home.path());
        cmd.args(head).args(tail);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{head:?} {tail:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn stdin(&self, args: &[&str], input: &str) -> String {
        let mut cmd = bin();
        isolate(&mut cmd, self.home.path());
        cmd.args(args).write_stdin(input.to_string());
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// `crystalline domain <args> --config <the fixture's config>`.
    fn domain(&self, args: &[&str]) -> String {
        let config = self.config.to_str().unwrap().to_string();
        let mut all: Vec<&str> = args.to_vec();
        all.push("--config");
        all.push(&config);
        self.run(&["domain"], &all)
    }

    /// The same, for a command that must fail. Returns stderr.
    fn domain_err(&self, args: &[&str]) -> String {
        let mut cmd = bin();
        isolate(&mut cmd, self.home.path());
        cmd.arg("domain")
            .args(args)
            .arg("--config")
            .arg(&self.config);
        let out = cmd.output().unwrap();
        assert!(
            !out.status.success(),
            "domain {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8(out.stderr).unwrap()
    }
}

/// The whole round trip, in the order an operator would walk it: close a
/// domain, invite somebody, list, remove, hand it on, open it again.
#[test]
fn the_membership_lifecycle_round_trips() {
    let fx = Fixture::new();

    // A domain nobody closed is shared, and the listing says what to do next
    // rather than showing an empty table.
    let shared = fx.domain(&["members", "eng", "list"]);
    assert!(shared.contains("is shared"), "{shared}");
    assert!(
        shared.contains("domain visibility eng private"),
        "it names the command that closes it: {shared}"
    );

    let closed = fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    assert!(closed.contains("private"), "{closed}");
    assert!(closed.contains("ada"), "{closed}");

    let invited = fx.domain(&["members", "eng", "add", "bob", "--level", "editor"]);
    assert!(invited.contains("bob"), "{invited}");
    assert!(invited.contains("editor"), "{invited}");
    assert!(
        invited.contains("with Reflection default"),
        "a new member starts at default: {invited}"
    );

    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(
        listed.contains("private, owned by 'ada'"),
        "the owner is stated, not folded into the table: {listed}"
    );
    assert!(listed.contains("NAME"), "{listed}");
    assert!(listed.contains("LEVEL"), "{listed}");
    assert!(listed.contains("REFLECTION"), "{listed}");
    assert!(listed.contains("bob"), "{listed}");
    assert!(listed.contains("1 member of 'eng'"), "{listed}");
    for line in listed.lines() {
        assert_eq!(line, line.trim_end(), "no line carries trailing whitespace");
    }

    // The level moves without an intervening removal: the same verb states
    // what the membership should be.
    fx.domain(&["members", "eng", "add", "bob", "--level", "manager"]);
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("manager"), "{listed}");
    assert!(!listed.contains("editor"), "{listed}");

    let removed = fx.domain(&["members", "eng", "remove", "bob"]);
    assert!(removed.contains("Removed 'bob'"), "{removed}");
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("Nobody else is invited"), "{listed}");

    // Handing the domain on, and the old owner keeping nothing.
    let handed = fx.domain(&["transfer", "eng", "bob"]);
    assert!(handed.contains("belongs to 'bob'"), "{handed}");
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("owned by 'bob'"), "{listed}");
    assert!(
        !listed.contains("ada"),
        "the previous owner keeps nothing: {listed}"
    );

    // And opening it again forgets who was invited.
    fx.domain(&["members", "eng", "add", "ada", "--level", "viewer"]);
    let opened = fx.domain(&["visibility", "eng", "default"]);
    assert!(opened.contains("shared"), "{opened}");
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("is shared"), "{listed}");
}

/// Closing a domain that is already closed changes nothing, and says so.
///
/// The write used to replace the acl row, so a second `visibility ... private`
/// handed the domain to whoever the second call named and dropped the previous
/// owner - who holds no membership row by construction - to no access at all.
/// This verb states a visibility; `domain transfer` is the one that changes an
/// owner, and the line points at it.
#[test]
fn closing_an_already_private_domain_changes_nothing() {
    let fx = Fixture::new();
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    fx.domain(&["members", "eng", "add", "bob", "--level", "editor"]);

    let again = fx.domain(&["visibility", "eng", "private", "--owner", "bob"]);
    assert!(
        again.contains("already private") && again.contains("'ada'"),
        "the line says nothing changed and names the owner it kept: {again}"
    );
    assert!(
        again.contains("domain transfer eng"),
        "and names the verb that does change one: {again}"
    );

    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(
        listed.contains("owned by 'ada'"),
        "the owner is untouched: {listed}"
    );
    assert!(
        listed.contains("bob") && listed.contains("editor"),
        "and so is the membership list: {listed}"
    );
}

/// The refusals, all of which have to happen before anything is written: a
/// domain nobody registered, an account nobody has, a missing owner and the
/// owner that is not a membership row.
#[test]
fn the_refusals_name_the_fix() {
    let fx = Fixture::new();

    // A name nobody registered, on every verb. The accounts database cannot
    // know which domains exist, so this check is the CLI's own.
    for args in [
        vec!["members", "ghost", "list"],
        vec!["visibility", "ghost", "private", "--owner", "ada"],
        vec!["transfer", "ghost", "ada"],
    ] {
        let err = fx.domain_err(&args);
        assert!(
            err.contains("no domain named 'ghost'"),
            "{args:?} must refuse an unregistered name: {err}"
        );
    }

    // Closing a domain needs somebody to own it, and the CLI has no calling
    // account to fall back on.
    let err = fx.domain_err(&["visibility", "eng", "private"]);
    assert!(err.contains("--owner"), "{err}");
    let err = fx.domain_err(&["visibility", "eng", "private", "--owner", "ghost"]);
    assert!(err.contains("no account named 'ghost'"), "{err}");

    // And a shared domain has no owner to pass one for.
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    let err = fx.domain_err(&["visibility", "eng", "default", "--owner", "ada"]);
    assert!(err.contains("no owner"), "{err}");

    // The owner holds no membership row; the refusal names the verb that does
    // change who it is.
    let err = fx.domain_err(&["members", "eng", "remove", "ada"]);
    assert!(err.contains("domain transfer eng"), "{err}");
    let err = fx.domain_err(&["members", "eng", "remove", "bob"]);
    assert!(err.contains("not a member"), "{err}");
    let err = fx.domain_err(&["members", "eng", "add", "ghost"]);
    assert!(err.contains("no account named 'ghost'"), "{err}");
}

/// A domain whose owner's account was removed belongs to nobody, and the
/// listing says exactly that rather than printing an empty name.
#[test]
fn a_domain_whose_owner_was_removed_is_rendered_as_owned_by_nobody() {
    let fx = Fixture::new();
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    fx.domain(&["members", "eng", "add", "bob", "--level", "editor"]);
    fx.run(&["users", "remove", "ada"], &[]);

    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(
        listed.contains("owned by nobody"),
        "an un-named owner is stated, never rendered as an empty name: {listed}"
    );
    assert!(
        listed.contains("domain transfer eng"),
        "and the way out is named: {listed}"
    );
    assert!(
        listed.contains("bob"),
        "the people invited into it keep their levels: {listed}"
    );

    // Which is exactly what the transfer verb is for.
    fx.domain(&["transfer", "eng", "bob"]);
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("owned by 'bob'"), "{listed}");
}

/// A domain can be registered private in one step, owned by the account named
/// with it - the personal-domain case, with no window in which it is shared.
#[test]
fn a_domain_can_be_registered_private() {
    let fx = Fixture::new();

    fx.domain(&["add", "vault", "--virtual", "--private", "--owner", "ada"]);
    let listed = fx.domain(&["members", "vault", "list"]);
    assert!(listed.contains("private, owned by 'ada'"), "{listed}");

    // The account is resolved before anything is registered, so a name nobody
    // has an account for leaves no domain behind.
    let err = fx.domain_err(&["add", "attic", "--virtual", "--private", "--owner", "ghost"]);
    assert!(err.contains("no account named 'ghost'"), "{err}");
    let err = fx.domain_err(&["members", "attic", "list"]);
    assert!(
        err.contains("no domain named 'attic'"),
        "nothing was registered: {err}"
    );

    // `--private` without an owner is refused by the parser, before any work.
    let mut cmd = bin();
    isolate(&mut cmd, fx.home.path());
    let out = cmd
        .args(["domain", "add", "attic", "--virtual", "--private"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--owner"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--private` on a domain that was already registered is refused before
/// anything is printed: plain, today's message on stderr and nothing on
/// stdout; with `--json`, one error object on stdout. Non-zero either way,
/// and the domain stays shared.
#[test]
fn private_on_an_adopted_domain_is_refused_before_any_output() {
    let fx = Fixture::new();
    let folder = fx.home.path().join("kb");
    let add = |json: bool| {
        let mut cmd = bin();
        isolate(&mut cmd, fx.home.path());
        if json {
            cmd.arg("--json");
        }
        cmd.args(["domain", "add", "eng"])
            .arg(&folder)
            .args(["--no-sync", "--private", "--owner", "ada", "--config"])
            .arg(&fx.config);
        cmd.output().unwrap()
    };

    let plain = add(false);
    assert!(!plain.status.success());
    assert!(
        plain.stdout.is_empty(),
        "no success output first: {}",
        String::from_utf8_lossy(&plain.stdout)
    );
    let stderr = String::from_utf8_lossy(&plain.stderr);
    assert!(
        stderr.contains(
            "domain 'eng' was already registered, so --private changed nothing; close an existing domain with: crystalline domain visibility eng private --owner ada"
        ),
        "{stderr}"
    );

    let json = add(true);
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    assert!(
        !String::from_utf8_lossy(&json.stderr).contains("Error:"),
        "the JSON object is the whole answer: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("stdout is exactly one JSON document");
    assert_eq!(value["ok"], false, "{value}");
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("--private changed nothing"),
        "{value}"
    );

    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(
        !listed.contains("private, owned by"),
        "still shared: {listed}"
    );
}

/// `--json` on the listing stays one parseable document, and says the same
/// things the table does.
#[test]
fn the_json_listing_carries_the_owner_and_the_levels() {
    let fx = Fixture::new();
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    fx.domain(&["members", "eng", "add", "bob", "--level", "manager"]);

    let mut cmd = bin();
    isolate(&mut cmd, fx.home.path());
    let out = cmd
        .args(["--json", "domain", "members", "eng", "list", "--config"])
        .arg(&fx.config)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["domain"], serde_json::json!("eng"));
    assert_eq!(value["visibility"], serde_json::json!("private"));
    assert_eq!(value["owner"], serde_json::json!("ada"));
    assert_eq!(value["members"][0]["principal"], serde_json::json!("bob"));
    assert_eq!(value["members"][0]["level"], serde_json::json!("manager"));

    // A shared domain reports no owner as `null`, never as an empty name.
    fx.domain(&["visibility", "eng", "default"]);
    let mut cmd = bin();
    isolate(&mut cmd, fx.home.path());
    let out = cmd
        .args(["--json", "domain", "members", "eng", "list", "--config"])
        .arg(&fx.config)
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["visibility"], serde_json::json!("shared"));
    assert_eq!(value["owner"], serde_json::json!(null));
    assert!(value["members"].as_array().unwrap().is_empty());
}

/// The help text says who this command acts as. It is the one thing a reader
/// cannot infer from the verbs: these commands answer to no web role, because
/// whoever runs them already holds the files.
#[test]
fn the_help_says_the_machine_operator_administers_every_domain() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        ["domain", "members", "--help"],
        ["domain", "visibility", "--help"],
        ["domain", "transfer", "--help"],
    ] {
        let mut cmd = bin();
        isolate(&mut cmd, home.path());
        let out = cmd.args(args).output().unwrap();
        let text = String::from_utf8_lossy_owned(out.stdout);
        assert!(
            text.contains("machine operator administers every domain"),
            "{args:?} must say who it acts as: {text}"
        );
    }
}

/// **Unregistering a domain from the CLI retires its visibility and membership
/// records too, so the name comes back shared and ownerless.**
///
/// The CLI used to be a config-file edit of its own that never reached the
/// engine, so the `domain_acl` and `domain_member` rows outlived every removal.
/// That is an access-control inheritance rather than untidy rows: a domain
/// later registered under the same name came back private, owned by whoever
/// owned the old one and invisible to everybody else on the instance. It goes
/// through `Engine::unregister_domain` now, as the machine owner, which is the
/// same entry point the JSON API and the MCP tool call.
#[test]
fn removing_a_domain_retires_its_membership_so_the_name_comes_back_shared() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("kb");
    let dir = dir.to_str().unwrap().to_string();

    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    fx.domain(&["members", "eng", "add", "bob", "--level", "manager"]);
    let closed = fx.domain(&["members", "eng", "list"]);
    assert!(closed.contains("private, owned by 'ada'"), "{closed}");

    fx.domain(&["remove", "eng"]);
    // A different folder registered under the freed name: a new domain, and
    // whoever registers it must not inherit the old one's owner and members.
    fx.domain(&["add", "eng", &dir, "--no-sync"]);

    let reopened = fx.domain(&["members", "eng", "list"]);
    assert!(
        reopened.contains("is shared"),
        "a re-added name comes back shared: {reopened}"
    );
    assert!(
        !reopened.contains("ada"),
        "and inherits no owner: {reopened}"
    );
    assert!(!reopened.contains("bob"), "and no members: {reopened}");
}

/// **A read-only instance's registry is frozen along with its knowledge.**
///
/// A consequence of routing the CLI through `Engine::unregister_domain` rather
/// than a config edit of its own, and worth pinning rather than discovering:
/// `crystalline domain remove` now follows `service.read_only` the way
/// `crystalline config set` and `crystalline write` already do. Unregistering a
/// domain mutates the config, which is exactly what that mode freezes.
#[test]
fn a_read_only_instance_refuses_to_unregister_a_domain() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("kb");
    std::fs::write(
        &fx.config,
        format!(
            "domains:\n  eng:\n    path: {}\nservice:\n  read_only: true\n",
            dir.display()
        ),
    )
    .unwrap();

    let refused = fx.domain_err(&["remove", "eng"]);
    assert!(
        refused.contains("read-only"),
        "the refusal says which mode did it: {refused}"
    );
    assert!(
        dir.join("MANIFEST.md").exists(),
        "and nothing was touched on disk"
    );
}

/// A file domain's files are never deleted by a removal, so `--purge` is inert
/// on one, and the removal says the files were kept.
#[test]
fn removing_a_file_domain_keeps_its_files_and_says_so() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("kb");
    let removed = fx.domain(&["remove", "eng"]);
    assert!(
        removed.contains("eng"),
        "the removal names the domain: {removed}"
    );
    assert!(
        dir.join("MANIFEST.md").exists(),
        "the files stay exactly where they were"
    );
}

/// Per-domain Reflection collaborators from the command line: on a shared
/// domain as on a private one, listed beside the members, refused on a
/// domain kept in the database.
#[test]
fn cli_members_reflection_add_and_remove() {
    let fx = Fixture::new();

    let added = fx.domain(&[
        "members",
        "eng",
        "reflection",
        "add",
        "bob",
        "--level",
        "contributor",
    ]);
    assert!(added.contains("'bob'"), "{added}");
    assert!(added.contains("contributor"), "{added}");
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("Reflection collaborators"), "{listed}");
    assert!(listed.contains("bob"), "{listed}");

    // The level moves with the same verb, and it works on a private domain.
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    fx.domain(&[
        "members",
        "eng",
        "reflection",
        "add",
        "bob",
        "--level",
        "reader",
    ]);
    let mut cmd = bin();
    isolate(&mut cmd, fx.home.path());
    let out = cmd
        .args(["--json", "domain", "members", "eng", "list", "--config"])
        .arg(&fx.config)
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value["reflection_collaborators"][0]["account"],
        serde_json::json!("bob")
    );
    assert_eq!(
        value["reflection_collaborators"][0]["level"],
        serde_json::json!("reader")
    );
    assert!(value["members"].as_array().unwrap().is_empty(), "{value}");

    // `default` grants nothing and is no level here; an unknown account is
    // refused with the fix.
    let err = fx.domain_err(&[
        "members",
        "eng",
        "reflection",
        "add",
        "bob",
        "--level",
        "default",
    ]);
    assert!(err.contains("default"), "{err}");
    let err = fx.domain_err(&[
        "members",
        "eng",
        "reflection",
        "add",
        "nobody",
        "--level",
        "reader",
    ]);
    assert!(err.contains("no account named 'nobody'"), "{err}");

    let removed = fx.domain(&["members", "eng", "reflection", "remove", "bob"]);
    assert!(removed.contains("'bob'"), "{removed}");
    let err = fx.domain_err(&["members", "eng", "reflection", "remove", "bob"]);
    assert!(err.contains("holds no Reflection grant"), "{err}");

    // A domain kept in the database takes none.
    fx.domain(&["add", "notes", "--virtual", "--no-sync"]);
    let err = fx.domain_err(&[
        "members",
        "notes",
        "reflection",
        "add",
        "bob",
        "--level",
        "reader",
    ]);
    assert!(
        err.contains("Reflection collaborators are for domains kept as files"),
        "{err}"
    );
}

/// On a domain a GitHub App governs, `domain visibility <name> default`
/// drops only the private record, the same as the Members card: the
/// membership rows are kept for the day the App is removed.
#[test]
fn cli_shared_on_a_governed_domain_keeps_its_membership_rows() {
    let fx = Fixture::new();
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    fx.domain(&["members", "eng", "add", "bob", "--level", "viewer"]);
    // The domain gets a GitHub origin, and the App comes from the
    // environment: GitHub governs it now.
    let domain_dir = fx.home.path().join("kb");
    std::fs::write(
        &fx.config,
        format!(
            "github:\n  enabled: true\n  api_url: http://127.0.0.1:1\ndomains:\n  eng:\n    path: {}\n    origin:\n      repo: acme/kb\n",
            domain_dir.display()
        ),
    )
    .unwrap();
    let mut cmd = with_app(&fx);
    cmd.args(["domain", "visibility", "eng", "default", "--config"])
        .arg(&fx.config);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("kept"), "{stdout}");
    // Closed again, the domain finds bob where he was.
    fx.domain(&["visibility", "eng", "private", "--owner", "ada"]);
    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(listed.contains("bob"), "{listed}");
}

/// A `crystalline` child in the fixture's home with a GitHub App taken from
/// the environment, so every domain with an origin is governed.
fn with_app(fx: &Fixture) -> Command {
    let mut cmd = bin();
    isolate(&mut cmd, fx.home.path());
    cmd.env(
        "CRYSTALLINE_TEST_TOKEN_STORE_DIR",
        fx.home.path().join("tokens"),
    )
    .env("CRYSTALLINE_GITHUB_APP_ID", "7")
    .env("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.test")
    .env(
        "CRYSTALLINE_GITHUB_APP_PRIVATE_KEY",
        include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem"),
    );
    cmd
}

/// On a governed domain, `private` records the reserved machine owner, never
/// an account: GitHub decides access. An `--owner` is not used, and the
/// command says so.
#[test]
fn cli_private_on_a_governed_domain_names_no_account() {
    let fx = Fixture::new();
    let domain_dir = fx.home.path().join("kb");
    std::fs::write(
        &fx.config,
        format!(
            "github:\n  enabled: true\n  api_url: http://127.0.0.1:1\ndomains:\n  eng:\n    path: {}\n    origin:\n      repo: acme/kb\n",
            domain_dir.display()
        ),
    )
    .unwrap();
    let out = with_app(&fx)
        .args([
            "domain",
            "visibility",
            "eng",
            "private",
            "--owner",
            "ada",
            "--config",
        ])
        .arg(&fx.config)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("--owner ada was not used"), "{stdout}");
    let mut list = with_app(&fx);
    list.args(["--json", "domain", "members", "eng", "list", "--config"])
        .arg(&fx.config);
    let out = list.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed["owner"], "owner", "{listed}");
}

/// A domain defined in the environment with an origin is governed too (the
/// engine counts every domain with an origin it knows, whether the config
/// file or the environment brought it), and its readers come from
/// `_READERS` on every start: with no daemon running, the CLI's own read
/// refuses a visibility change with the variable's name.
#[test]
fn cli_visibility_on_an_env_defined_governed_domain_names_the_variable() {
    let fx = Fixture::new();
    let team = fx.home.path().join("team");
    fx.run(
        &["domain", "init"],
        &[team.to_str().unwrap(), "--name", "team"],
    );
    for side in ["private", "default"] {
        let mut cmd = with_app(&fx);
        cmd.env("CRYSTALLINE_DOMAIN_TEAM", &team)
            .env("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/team")
            .args(["domain", "visibility", "team", side, "--config"])
            .arg(&fx.config);
        let out = cmd.output().unwrap();
        assert!(!out.status.success(), "{side}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Set by CRYSTALLINE_DOMAIN_TEAM_READERS; change it there."),
            "{side}: {stderr}"
        );
    }
}

/// Without a GitHub App the `_READERS` variable changes nothing, so an
/// env-defined domain with an origin takes a visibility change as any
/// other domain does.
#[test]
fn without_an_app_an_env_defined_domain_takes_a_visibility_change() {
    let fx = Fixture::new();
    let team = fx.home.path().join("team");
    fx.run(
        &["domain", "init"],
        &[team.to_str().unwrap(), "--name", "team"],
    );
    let mut cmd = bin();
    isolate(&mut cmd, fx.home.path());
    cmd.env("CRYSTALLINE_DOMAIN_TEAM", &team)
        .env("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/team")
        .args([
            "domain",
            "visibility",
            "team",
            "private",
            "--owner",
            "ada",
            "--config",
        ])
        .arg(&fx.config);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("owned by 'ada'"), "{stdout}");
}

/// Run `crystalline <args>` in `env` without the GitHub App variables,
/// with `stdin` when given, returning (success, stdout, stderr).
#[cfg(unix)]
fn in_daemon_env(
    env: &crate::daemon_env::DaemonEnv,
    args: &[&str],
    stdin: Option<&str>,
) -> (bool, String, String) {
    in_daemon_env_with(env, &[], args, stdin)
}

/// [`in_daemon_env`] with `vars` set on the command.
#[cfg(unix)]
fn in_daemon_env_with(
    env: &crate::daemon_env::DaemonEnv,
    vars: &[(&str, &str)],
    args: &[&str],
    stdin: Option<&str>,
) -> (bool, String, String) {
    use std::io::Write as _;
    let mut cmd = crate::common::crystalline_std();
    env.apply(&mut cmd);
    for (name, value) in vars {
        cmd.env(name, value);
    }
    for name in [
        "CRYSTALLINE_GITHUB_APP_ID",
        "CRYSTALLINE_GITHUB_APP_CLIENT_ID",
        "CRYSTALLINE_GITHUB_APP_PRIVATE_KEY",
    ] {
        cmd.env_remove(name);
    }
    let mut child = cmd
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut pipe = child.stdin.take().unwrap();
        if let Some(input) = stdin {
            pipe.write_all(input.as_bytes()).unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// An environment whose daemon has the GitHub App from its own
/// environment (a service manager's environment file), while every command
/// run here has none: a domain `eng` with a GitHub origin, private to `ada`
/// with `bob` invited, set up before the daemon started. `env_team` also
/// defines the domain `team` in the daemon's environment with an origin.
#[cfg(unix)]
fn governed_by_the_daemon_only(tag: &str, env_team: bool) -> crate::daemon_env::DaemonEnv {
    let mut env = crate::daemon_env::DaemonEnv::new(tag);
    let kb = env.dir.join("kb");
    let team = env.dir.join("team");
    for (dir, name) in [(&kb, "eng"), (&team, "team")] {
        let (ok, _, err) = in_daemon_env(
            &env,
            &["domain", "init", dir.to_str().unwrap(), "--name", name],
            None,
        );
        assert!(ok, "{err}");
    }
    std::fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    std::fs::write(
        env.config_path(),
        format!(
            "github:\n  enabled: true\n  api_url: http://127.0.0.1:1\ndomains:\n  eng:\n    path: {}\n    origin:\n      repo: acme/kb\n",
            kb.display()
        ),
    )
    .unwrap();
    for (name, role) in [("ada", "editor"), ("bob", "viewer"), ("carol", "viewer")] {
        let (ok, _, err) = in_daemon_env(
            &env,
            &["users", "add", name, "--role", role, "--password-stdin"],
            Some("s3cret\n"),
        );
        assert!(ok, "{err}");
    }
    for args in [
        &["domain", "visibility", "eng", "private", "--owner", "ada"][..],
        &[
            "domain", "members", "eng", "add", "bob", "--level", "viewer",
        ][..],
    ] {
        let (ok, _, err) = in_daemon_env(&env, args, None);
        assert!(ok, "{args:?}: {err}");
    }
    let key = include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem");
    let team_path = team.display().to_string();
    let mut daemon_env = vec![
        ("CRYSTALLINE_GITHUB_APP_ID", "7"),
        ("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.test"),
        ("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", key),
    ];
    if env_team {
        daemon_env.push(("CRYSTALLINE_DOMAIN_TEAM", team_path.as_str()));
        daemon_env.push(("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/team"));
    }
    env.serve_with_env(&daemon_env);
    env
}

/// "Governed" is the running daemon's answer, not the CLI's own read of
/// its environment: a daemon with the App from its environment file governs
/// `eng`, and the CLI run in a plain shell has no App at all. Shared keeps
/// the membership rows, private records the reserved owner, and a
/// membership change is refused the way the web API refuses it.
#[cfg(unix)]
#[test]
fn the_daemon_decides_governed_when_the_cli_has_no_app() {
    let env = governed_by_the_daemon_only("gov", false);
    let (ok, out, err) = in_daemon_env(
        &env,
        &["--json", "domain", "visibility", "eng", "default"],
        None,
    );
    assert!(ok, "{err}");
    let shared: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(shared["members_kept"], true, "{shared}");

    let (ok, _, err) = in_daemon_env(
        &env,
        &[
            "domain", "members", "eng", "add", "carol", "--level", "viewer",
        ],
        None,
    );
    assert!(!ok);
    assert!(
        err.contains("access to this domain follows GitHub; change it on GitHub"),
        "{err}"
    );
    let (ok, _, err) = in_daemon_env(&env, &["domain", "members", "eng", "remove", "bob"], None);
    assert!(!ok);
    assert!(err.contains("follows GitHub"), "{err}");

    let (ok, _, err) = in_daemon_env(
        &env,
        &["domain", "visibility", "eng", "private", "--owner", "ada"],
        None,
    );
    assert!(ok, "{err}");
    let (ok, out, err) = in_daemon_env(&env, &["--json", "domain", "members", "eng", "list"], None);
    assert!(ok, "{err}");
    let listed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(listed["owner"], "owner", "{listed}");
    let members = listed["members"].to_string();
    assert!(members.contains("bob"), "{listed}");
    assert!(!members.contains("carol"), "{listed}");
}

/// A domain the daemon's environment defines with an origin applies its
/// readers again on every start, so a visibility change from the CLI would
/// not last: refused with the variable's name, both ways, as the web API
/// refuses it.
#[cfg(unix)]
#[test]
fn visibility_on_an_env_defined_governed_domain_names_the_variable() {
    let env = governed_by_the_daemon_only("envr", true);
    // The command sees the domain as the daemon does, without the App.
    let team = env.dir.join("team").display().to_string();
    let vars = [
        ("CRYSTALLINE_DOMAIN_TEAM", team.as_str()),
        ("CRYSTALLINE_DOMAIN_TEAM_ORIGIN", "acme/team"),
    ];
    for args in [
        &["domain", "visibility", "team", "private"][..],
        &["domain", "visibility", "team", "default"][..],
    ] {
        let (ok, _, err) = in_daemon_env_with(&env, &vars, args, None);
        assert!(!ok, "{args:?}");
        assert!(
            err.contains("Set by CRYSTALLINE_DOMAIN_TEAM_READERS; change it there."),
            "{args:?}: {err}"
        );
    }
}
