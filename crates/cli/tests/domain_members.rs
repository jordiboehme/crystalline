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

mod common;
use common::isolate;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
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

    let listed = fx.domain(&["members", "eng", "list"]);
    assert!(
        listed.contains("private, owned by 'ada'"),
        "the owner is stated, not folded into the table: {listed}"
    );
    assert!(listed.contains("NAME"), "{listed}");
    assert!(listed.contains("LEVEL"), "{listed}");
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
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            text.contains("machine operator administers every domain"),
            "{args:?} must say who it acts as: {text}"
        );
    }
}
