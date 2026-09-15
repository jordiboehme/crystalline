//! End to end for `crystalline domain review`: the switch that decides whether
//! a domain reviews changes before they land, and the per-actor answer that
//! ends the drafts on the way back out.
//!
//! Every child runs with its own `HOME` and XDG base directories (`isolate`),
//! so the index, the config and the overlay journal all land in a temp state
//! directory rather than in the developer's own.
//!
//! The property under test: the machine operator is never told what happened
//! only after it happened. The verb draws the plan first and prints it, and the
//! change is refused until the answer covers every actor the plan named.

use assert_cmd::Command;

mod common;
use common::isolate;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// An isolated home holding a file domain `team` in review mode, with one draft
/// in it that the machine owner wrote.
struct Fixture {
    home: tempfile::TempDir,
    config: std::path::PathBuf,
    domain_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture::build(true)
    }

    /// The same domain, taking changes directly: what a domain looks like
    /// before anybody turns review mode on.
    fn plain() -> Fixture {
        Fixture::build(false)
    }

    fn build(review: bool) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.yaml");
        let domain_dir = home.path().join("team");
        let fx = Fixture {
            home,
            config,
            domain_dir,
        };
        fx.ok(&[
            "domain",
            "init",
            fx.domain_dir.to_str().unwrap(),
            "--name",
            "team",
        ]);
        fx.ok(&[
            "domain",
            "add",
            "team",
            fx.domain_dir.to_str().unwrap(),
            "--config",
            fx.config.to_str().unwrap(),
        ]);
        // Review mode is written straight into the configuration here, because
        // turning it on through the verb needs a GitHub origin and a pull this
        // suite has no forge for. What this test is about is the way back out.
        if review {
            let text = std::fs::read_to_string(&fx.config).unwrap();
            std::fs::write(
                &fx.config,
                text.replace("  team:\n", "  team:\n    review: overlay\n"),
            )
            .unwrap();
        }
        fx
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = bin();
        isolate(&mut cmd, self.home.path());
        cmd.args(args);
        cmd.output().unwrap()
    }

    /// Run a command that must succeed, returning stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Run a command that must fail, returning stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            !out.status.success(),
            "{args:?} was expected to refuse but succeeded: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8(out.stderr).unwrap()
    }
}

#[test]
fn domain_review_preview_and_fold_round_trip() {
    let fx = Fixture::new();
    let config = fx.config.to_str().unwrap().to_string();

    // A write on a domain in review mode joins the machine owner's own draft
    // and the folder stays as the team left it.
    let written = fx.ok(&[
        "--json",
        "write",
        "team",
        "Retry backoff",
        "--content",
        "- [decision] the retry queue doubles its backoff #team",
        "--config",
        &config,
    ]);
    let written: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(written["draft"], serde_json::json!(true), "{written}");
    assert!(
        !fx.domain_dir.join("retry-backoff.md").exists(),
        "the folder says what the team reviewed, not what the owner is drafting"
    );

    // The plan first: named, counted, and nothing changed by asking.
    let plan = fx.ok(&[
        "--json", "domain", "review", "team", "direct", "--config", &config,
    ]);
    let plan: serde_json::Value = serde_json::from_str(&plan).unwrap();
    assert_eq!(plan["applied"], serde_json::json!(false));
    assert_eq!(
        plan["actors"][0]["actor"],
        serde_json::json!("owner"),
        "the machine owner is who the drafts belong to here: {plan}"
    );
    assert_eq!(plan["actors"][0]["entries"], serde_json::json!(1));
    assert!(
        !fx.domain_dir.join("retry-backoff.md").exists(),
        "and the preview wrote nothing"
    );

    // An answer that does not cover the actor the plan named is refused, and
    // the refusal names them.
    let refused = fx.fails(&[
        "domain",
        "review",
        "team",
        "direct",
        "--discard",
        "nobody",
        "--config",
        &config,
    ]);
    assert!(
        refused.contains("owner"),
        "the refusal names who is unaccounted for: {refused}"
    );
    assert!(
        !fx.domain_dir.join("retry-backoff.md").exists(),
        "and a refused answer changes nothing"
    );

    // With the answer, the draft is the team's engram.
    let done = fx.ok(&[
        "--json", "domain", "review", "team", "direct", "--fold", "owner", "--config", &config,
    ]);
    let done: serde_json::Value = serde_json::from_str(&done).unwrap();
    assert_eq!(done["applied"], serde_json::json!(true));
    assert_eq!(
        done["folded"],
        serde_json::json!([{ "actor": "owner", "written": 1, "deleted": 0 }]),
        "{done}"
    );
    let landed = std::fs::read_to_string(fx.domain_dir.join("retry-backoff.md")).unwrap();
    assert!(
        landed.contains("doubles its backoff"),
        "the draft is the file now: {landed}"
    );
    assert!(
        !std::fs::read_to_string(&fx.config)
            .unwrap()
            .contains("review: overlay"),
        "and the domain takes changes directly again"
    );

    // Which the very next write proves: it is in the folder, not in a draft.
    let after = fx.ok(&[
        "--json",
        "write",
        "team",
        "After the fold",
        "--content",
        "- [fact] written straight into the folder #team",
        "--config",
        &config,
    ]);
    let after: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert!(after["draft"].is_null(), "{after}");
    assert!(fx.domain_dir.join("after-the-fold.md").exists());
}

/// Turning review mode ON needs a GitHub origin, and the refusal says so rather
/// than leaving the operator to guess which of the mode's three requirements is
/// missing.
#[test]
fn enabling_review_without_an_origin_refuses_with_the_way_in() {
    let fx = Fixture::plain();
    let config = fx.config.to_str().unwrap().to_string();
    let refused = fx.fails(&["domain", "review", "team", "overlay", "--config", &config]);
    assert!(
        refused.contains("connect it to a GitHub repository"),
        "{refused}"
    );
}

/// Ending a reviewing domain from the command line: the operator's own drafts
/// go without a question, and a name nobody here is drafting under is refused.
///
/// The machine operator drafts as `owner`, so the removal asks them nothing
/// about their own unshared work - they are the one person in the room who
/// already knows. What `--end-drafts` is for is everybody else, and a name that
/// belongs to nobody is somebody meaning a different domain or a different
/// moment rather than an answer about this one.
#[test]
fn domain_remove_ends_the_owners_own_drafts_and_refuses_a_stranger() {
    let fx = Fixture::new();
    let config = fx.config.to_str().unwrap().to_string();
    fx.ok(&[
        "--json",
        "write",
        "team",
        "Retry backoff",
        "--content",
        "- [decision] the retry queue doubles its backoff #team",
        "--config",
        &config,
    ]);

    // A name nobody here drafts under. `carol` rather than `nobody`, because
    // the refusal sentence carries the word "nobody" on its own and an
    // assertion over that word would pass with the name dropped entirely.
    let refused = fx.fails(&[
        "domain",
        "remove",
        "team",
        "--end-drafts",
        "carol",
        "--config",
        &config,
    ]);
    assert!(
        refused.contains("carol"),
        "the refusal names who nobody is: {refused}"
    );

    let report = fx.ok(&["--json", "domain", "remove", "team", "--config", &config]);
    let report: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        report["drafts_swept"],
        serde_json::json!(1),
        "the operator's own draft went with the domain, unasked: {report}"
    );
}
