//! `crystalline doctor`'s domain name findings: a shadowed name, a name two
//! domains claim, an alias that does not resolve, a team domain that declares
//! no name, and links spelled with a name only this machine uses, which
//! `--fix` respells. Also the empty index row a domain removed by an older
//! version left behind, which `--fix` drops.
//!
//! Every run is isolated under a scratch `HOME` (with the keychain seam set),
//! and the configuration and index live in its state directory.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};

use crate::common::{isolate, isolated_state_dir};

fn bin() -> Command {
    crate::common::crystalline()
}

/// A scratch home, and the config and index paths inside its state
/// directory.
struct Machine {
    home: tempfile::TempDir,
    config: PathBuf,
    db: PathBuf,
}

impl Machine {
    fn new() -> Machine {
        let home = tempfile::tempdir().unwrap();
        let state = isolated_state_dir(home.path());
        std::fs::create_dir_all(&state).unwrap();
        Machine {
            config: state.join("config.yaml"),
            db: state.join("index.db"),
            home,
        }
    }

    /// A domain folder under the home, with a MANIFEST that declares
    /// `declared` as its `domain_name` when given.
    fn folder(&self, name: &str, declared: Option<&str>) -> PathBuf {
        let root = self.home.path().join("kb").join(name);
        std::fs::create_dir_all(&root).unwrap();
        let line = declared
            .map(|d| format!("domain_name: {d}\n"))
            .unwrap_or_default();
        std::fs::write(
            root.join("MANIFEST.md"),
            format!(
                "---\ntype: manifest\ntitle: {name}\npermalink: manifest\n{line}tags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n"
            ),
        )
        .unwrap();
        root
    }

    /// Write the configuration: `(local name, folder, extra entry lines)`,
    /// every entry explicitly named.
    fn configure(&self, domains: &[(&str, &Path, &str)]) {
        let mut yaml = String::from("domains:\n");
        for (name, root, extra) in domains {
            yaml.push_str(&format!(
                "  {name}:\n    path: {}\n    name_origin: explicit\n{extra}",
                root.display()
            ));
        }
        std::fs::write(&self.config, yaml).unwrap();
    }

    fn cmd(&self) -> Command {
        let mut cmd = bin();
        isolate(&mut cmd, self.home.path());
        cmd.env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1")
            .env_remove("COPILOT_HOME");
        cmd
    }

    fn sync(&self) {
        self.cmd()
            .args(["sync", "--config"])
            .arg(&self.config)
            .arg("--db")
            .arg(&self.db)
            .assert()
            .success();
    }

    /// `doctor` with `args`, answering the exit code and stdout.
    fn doctor(&self, args: &[&str]) -> (i32, String) {
        let out = self
            .cmd()
            .args(args)
            .arg("doctor")
            .arg("--config")
            .arg(&self.config)
            .arg("--db")
            .arg(&self.db)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8(out.stdout).unwrap(),
        )
    }

    fn doctor_json(&self) -> (i32, Value) {
        let (code, out) = self.doctor(&["--json"]);
        let report: Value = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
        (code, report)
    }

    fn doctor_fix_json(&self) -> (i32, String) {
        let out = self
            .cmd()
            .args(["--json", "doctor", "--fix", "--config"])
            .arg(&self.config)
            .arg("--db")
            .arg(&self.db)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8(out.stdout).unwrap(),
        )
    }
}

fn engram(title: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n",
        title.to_lowercase()
    )
}

/// `eng` says its name is `ops`, the local name of another domain: shadowed.
/// `a` and `b` both say `shared`: contested. `b`'s alias `ops` is taken:
/// dropped. All three are warnings, so doctor still exits 0, and each line
/// names its next step.
#[test]
fn shadowed_contested_and_dropped_names_are_warnings() {
    let m = Machine::new();
    let ops = m.folder("ops", None);
    let eng = m.folder("eng", Some("ops"));
    let a = m.folder("a", Some("shared"));
    let b = m.folder("b", Some("shared"));
    m.configure(&[
        ("ops", &ops, ""),
        ("eng", &eng, ""),
        ("a", &a, ""),
        ("b", &b, "    aliases:\n      - ops\n"),
    ]);
    m.sync();

    let (code, report) = m.doctor_json();
    assert_eq!(code, 0, "{report}");
    let names = &report["names"];
    assert_eq!(
        names["shadowed"],
        json!([{ "domain": "eng", "canonical": "ops", "held_by": "ops" }]),
        "{report}"
    );
    assert_eq!(
        names["conflicts"],
        json!([{ "name": "shared", "claimants": ["a", "b"] }]),
        "{report}"
    );
    assert_eq!(
        names["dropped_aliases"],
        json!([{ "domain": "b", "alias": "ops", "held_by": "ops" }]),
        "{report}"
    );

    let (code, human) = m.doctor(&[]);
    assert_eq!(code, 0, "{human}");
    assert!(
        human.contains(
            "domain 'eng' says its name is 'ops', but 'ops' is another domain here; links \
             that name 'ops' reach that one. Rename one of them: crystalline domain rename \
             <domain> <new>"
        ),
        "{human}"
    );
    assert!(
        human.contains(
            "domains 'a' and 'b' both call themselves 'shared'; links that name 'shared' \
             reach neither. Rename one of them"
        ),
        "{human}"
    );
    assert!(
        human.contains("alias 'ops' of 'b' is ignored: 'ops' already names 'ops'"),
        "{human}"
    );
}

/// A team domain whose MANIFEST declares no name gets a hint that names the
/// local name to add; doctor still exits 0. One that declares a name does
/// not.
#[test]
fn a_team_domain_without_a_declared_name_gets_a_hint() {
    let m = Machine::new();
    let kb = m.folder("eng-knowledge", None);
    let hb = m.folder("handbook", Some("handbook"));
    m.configure(&[
        (
            "eng",
            &kb,
            "    origin:\n      repo: acme/eng-knowledge\n      branch: main\n",
        ),
        (
            "handbook",
            &hb,
            "    origin:\n      repo: acme/handbook\n      branch: main\n",
        ),
    ]);
    m.sync();

    let (code, report) = m.doctor_json();
    assert_eq!(code, 0, "{report}");
    assert_eq!(
        report["names"]["team_without_domain_name"],
        json!([{ "domain": "eng", "repo": "acme/eng-knowledge" }]),
        "{report}"
    );

    let (_, human) = m.doctor(&[]);
    assert!(
        human.contains(
            "team domain 'eng' declares no domain_name in its MANIFEST, so each colleague \
             may name it differently. Ask the owner to add `domain_name: eng` to the MANIFEST"
        ),
        "{human}"
    );
    assert!(!human.contains("team domain 'handbook'"), "{human}");
}

/// A link in `ops` names `eng` by its local name while the domain calls
/// itself `engineering`: a problem until fixed. `--fix` writes the domain's
/// name into the file, and the next run is clean for that finding.
#[test]
fn local_only_spellings_are_a_problem_until_fix_respells_them() {
    let m = Machine::new();
    let eng = m.folder("eng", Some("engineering"));
    std::fs::write(eng.join("runbook.md"), engram("Runbook", "The steps.")).unwrap();
    let ops = m.folder("ops", None);
    std::fs::write(
        ops.join("note.md"),
        engram(
            "Note",
            "See [[eng:runbook]] and crystalline://eng/runbook for the steps.",
        ),
    )
    .unwrap();
    m.configure(&[("eng", &eng, ""), ("ops", &ops, "")]);
    m.sync();

    let (code, report) = m.doctor_json();
    assert_eq!(code, 1, "{report}");
    assert_eq!(
        report["names"]["local_spellings"],
        json!([{
            "domain": "ops", "path": "note.md", "spelling": "eng",
            "canonical": "engineering", "count": 2
        }]),
        "{report}"
    );
    let (_, human) = m.doctor(&[]);
    assert!(
        human.contains(
            "2 links name a domain by a name only this machine uses; rerun with --fix to \
             write the domain's name instead"
        ),
        "{human}"
    );

    let (code, out) = m.doctor_fix_json();
    let report: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["names"]["fixed"], json!(2), "{report}");
    assert_eq!(report["names"]["local_spellings"], json!([]), "{report}");
    let text = std::fs::read_to_string(ops.join("note.md")).unwrap();
    assert!(
        text.contains("[[engineering:runbook]]")
            && text.contains("crystalline://engineering/runbook"),
        "{text}"
    );

    let (code, report) = m.doctor_json();
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["names"]["local_spellings"], json!([]), "{report}");
}

/// A domain dropped from the configuration by hand leaves its rows and its
/// row behind. `--fix` collects the rows and drops the row, so the name is
/// free for a rename or an adoption, and the next run no longer lists it.
#[test]
fn fix_drops_the_row_of_a_domain_nobody_registers() {
    let m = Machine::new();
    let eng = m.folder("eng", None);
    let gone = m.folder("gone", None);
    std::fs::write(gone.join("a.md"), engram("A", "Some text.")).unwrap();
    m.configure(&[("eng", &eng, ""), ("gone", &gone, "")]);
    m.sync();
    m.configure(&[("eng", &eng, "")]);

    let (code, out) = m.doctor_fix_json();
    let report: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, 0, "{report}");
    let row = report["orphaned_rows"]["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "gone")
        .cloned()
        .unwrap_or_else(|| panic!("{report}"));
    assert_eq!(row["collected"], true, "{row}");
    assert_eq!(row["row_dropped"], true, "{row}");

    let (code, report) = m.doctor_json();
    assert_eq!(code, 0, "{report}");
    assert!(
        report["orphaned_rows"]["domains"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["name"] != "gone"),
        "{report}"
    );
}
