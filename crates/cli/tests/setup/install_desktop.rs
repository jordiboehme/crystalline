//! `crystalline install claude-desktop`: the hollow extension is built on
//! this machine, pins the crystalline found on PATH and is recorded in
//! `<state_dir>/claude-desktop.json`.
//!
//! Every run passes `--skip-skills` except the skill step's own tests at the
//! end, which save the crystalline-intelligence zip into a temp Downloads
//! folder (or the state folder) and reveal it through the same seam. Every
//! child gets a PATH of its own, so the developer's own crystalline is
//! never the one pinned. Most runs pass `--no-open`; the ones that open go
//! through the debug build's open seam (`CRYSTALLINE_TEST_DESKTOP_OPEN`,
//! `fail` in the isolation environment), so no test opens Claude Desktop.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use sha2::{Digest, Sha256};

const FOOTER: &str =
    "Crystalline cannot see what Claude Desktop has installed. It only remembers what it did last.";

const BINARY_NAME: &str = if cfg!(windows) {
    "crystalline.exe"
} else {
    "crystalline"
};

/// A temp home plus a `bin` folder holding a fake executable crystalline.
struct Setup {
    home: tempfile::TempDir,
    bin: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        fake_binary(&bin);
        Setup { home, bin }
    }

    fn binary(&self) -> PathBuf {
        self.bin.join(BINARY_NAME)
    }

    fn state(&self) -> PathBuf {
        crate::common::isolated_state_dir(self.home.path())
    }

    fn bundle(&self) -> PathBuf {
        self.state().join("claude-desktop").join("crystalline.mcpb")
    }

    fn record_file(&self) -> PathBuf {
        self.state().join("claude-desktop.json")
    }

    /// `crystalline install claude-desktop --no-open --skip-skills` with
    /// PATH set to `path`.
    fn install(&self, path: &OsString) -> Command {
        let mut cmd = crate::common::crystalline();
        crate::common::isolate(&mut cmd, self.home.path());
        cmd.env("PATH", path)
            .args(["install", "claude-desktop", "--no-open", "--skip-skills"]);
        cmd
    }

    /// `crystalline install claude-desktop --skip-skills` with PATH set to
    /// `path`: it opens the extension through the test seam `seam`.
    fn install_opening(&self, path: &OsString, seam: &str) -> Command {
        let mut cmd = crate::common::crystalline();
        crate::common::isolate(&mut cmd, self.home.path());
        cmd.env("PATH", path)
            .env("CRYSTALLINE_TEST_DESKTOP_OPEN", seam)
            .args(["install", "claude-desktop", "--skip-skills"]);
        cmd
    }

    fn open_log(&self) -> PathBuf {
        self.home.path().join("open.log")
    }

    fn path_env(&self) -> OsString {
        std::env::join_paths([&self.bin]).unwrap()
    }

    fn record(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.record_file()).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

fn fake_binary(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let file = dir.join(BINARY_NAME);
    std::fs::write(&file, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn entry(zip_file: &Path, name: &str) -> Vec<u8> {
    let bytes = std::fs::read(zip_file).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut file = archive.by_name(name).unwrap();
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}

fn manifest(zip_file: &Path) -> serde_json::Value {
    serde_json::from_slice(&entry(zip_file, "manifest.json")).unwrap()
}

fn pinned(zip_file: &Path) -> String {
    manifest(zip_file)["server"]["mcp_config"]["env"]["CRYSTALLINE_BINARY"]
        .as_str()
        .unwrap()
        .to_string()
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

#[test]
fn the_extension_is_built_into_the_state_folder() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let bundle = s.bundle();
    for name in ["manifest.json", "icon.png", "server/shim.js"] {
        assert!(!entry(&bundle, name).is_empty(), "{name} is in the zip");
    }
    assert_eq!(pinned(&bundle), s.binary().display().to_string());
    assert_eq!(manifest(&bundle)["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_record_says_what_was_built() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let record = s.record();
    assert_eq!(record["format"], 1);
    let ext = &record["extension"];
    assert_eq!(ext["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ext["binary"], s.binary().display().to_string());
    let shim = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/mcpb/shim/shim.js"),
    )
    .unwrap();
    let hash: String = Sha256::digest(&shim)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(ext["shim_sha256"], hash);
    assert_eq!(ext["file"], s.bundle().display().to_string());
    chrono::DateTime::parse_from_rfc3339(ext["built_at"].as_str().unwrap())
        .expect("built_at is RFC 3339");
    assert_eq!(ext["opened"], false);
    assert!(record.get("skill").is_none_or(|v| v.is_null()), "{record}");
}

#[test]
fn with_no_crystalline_on_path_the_extension_runs_this_binary_and_says_so() {
    let s = Setup::new();
    let empty = s.home.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let assert = s
        .install(&std::env::join_paths([&empty]).unwrap())
        .assert()
        .success();
    let got = pinned(&s.bundle());
    let this = assert_cmd::cargo::cargo_bin("crystalline");
    assert_eq!(
        std::fs::canonicalize(&got).unwrap(),
        std::fs::canonicalize(&this).unwrap(),
        "the extension runs the binary that built it"
    );
    let out = stdout(&assert);
    let warning = format!(
        "crystalline is not on your PATH, so Claude Desktop gets this binary's own path ({got})"
    );
    assert!(out.contains(&warning), "{out}");
}

#[test]
fn a_versioned_folder_on_path_is_pinned_with_a_warning() {
    let s = Setup::new();
    let cellar = s.home.path().join("Cellar/crystalline/0.31.0/bin");
    fake_binary(&cellar);
    let assert = s
        .install(&std::env::join_paths([&cellar]).unwrap())
        .assert()
        .success();
    let program = cellar.join(BINARY_NAME).display().to_string();
    assert_eq!(pinned(&s.bundle()), program);
    let out = stdout(&assert);
    let warning = format!(
        "the crystalline on your PATH sits in a versioned folder ({program}); an upgrade may move it. Point PATH at a stable link such as /opt/homebrew/bin."
    );
    assert!(out.contains(&warning), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[lines.len() - 2],
        warning,
        "the warning comes before the footer"
    );
    assert_eq!(lines[lines.len() - 1], FOOTER);
}

#[test]
fn a_second_run_replaces_the_file_and_leaves_no_partial() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let first = s.record()["extension"]["built_at"]
        .as_str()
        .unwrap()
        .to_string();
    s.install(&s.path_env()).assert().success();
    let second = s.record()["extension"]["built_at"]
        .as_str()
        .unwrap()
        .to_string();
    let folder = s.bundle().parent().unwrap().to_path_buf();
    let names: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["crystalline.mcpb".to_string()], "{names:?}");
    let state_names: Vec<String> = std::fs::read_dir(s.state())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !state_names.iter().any(|n| n.ends_with(".partial")),
        "{state_names:?}"
    );
    let first = chrono::DateTime::parse_from_rfc3339(&first).unwrap();
    let second = chrono::DateTime::parse_from_rfc3339(&second).unwrap();
    assert!(second >= first, "{first} then {second}");
}

#[test]
fn no_open_prints_the_path_and_the_settings_route() {
    let s = Setup::new();
    let assert = s.install(&s.path_env()).assert().success();
    let expected = format!(
        "Built the Claude Desktop extension for Crystalline {}: {}\n\
         It runs {}.\n\
         To install it, open Claude Desktop, go to Settings > Extensions > Advanced settings > Install Extension... and pick this file.\n\
         {FOOTER}\n",
        env!("CARGO_PKG_VERSION"),
        s.bundle().display(),
        s.binary().display(),
    );
    assert_eq!(stdout(&assert), expected);
}

#[test]
fn json_reports_the_extension() {
    let s = Setup::new();
    let assert = s.install(&s.path_env()).arg("--json").assert().success();
    let out = stdout(&assert);
    let report: serde_json::Value = serde_json::from_str(out.trim()).expect("one JSON object");
    assert_eq!(report["target"], "claude-desktop");
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        report["extension"]["file"],
        s.bundle().display().to_string()
    );
    assert_eq!(
        report["extension"]["binary"],
        s.binary().display().to_string()
    );
    assert_eq!(report["extension"]["opened"], false);
    assert!(report["extension"]["open_error"].is_null());
    assert!(report["skill"].is_null());
    assert_eq!(report["notices"], serde_json::json!([]));
}

#[test]
fn flags_that_do_not_apply_are_refused() {
    let s = Setup::new();
    let cases: [(&[&str], &str); 3] = [
        (
            &["--project"],
            "--project does not apply to claude-desktop: Claude Desktop extensions are per user.",
        ),
        (
            &["--skip-hooks"],
            "--skip-hooks does not apply to claude-desktop: Claude Desktop has no hooks.",
        ),
        (
            &["--skip-mcp"],
            "--skip-mcp and --skip-skills together leave nothing to install.",
        ),
    ];
    for (flags, error) in cases {
        let assert = s.install(&s.path_env()).args(flags).assert().failure();
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
        assert!(stderr.contains(error), "{flags:?}: {stderr}");
        assert!(!s.record_file().exists(), "{flags:?} wrote no record");
        assert!(
            !s.bundle().parent().unwrap().exists(),
            "{flags:?} wrote no extension"
        );
    }
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, s.home.path());
    let assert = cmd
        .env("PATH", s.path_env())
        .args(["install", "claude-code", "--no-open"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("--no-open applies only to claude-desktop."),
        "{stderr}"
    );
    assert!(
        !s.home.path().join(".claude").exists(),
        "claude-code was not installed"
    );
}

#[test]
fn skip_mcp_keeps_the_recorded_extension() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let before = s.record()["extension"].clone();
    // Without --skip-skills: the skill step is the only one left to run, and
    // it may add to the record, so only the extension is compared.
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, s.home.path());
    let assert = cmd
        .env("PATH", s.path_env())
        .args(["install", "claude-desktop", "--no-open", "--skip-mcp"])
        .assert()
        .success();
    assert_eq!(s.record()["extension"], before);
    assert!(!stdout(&assert).contains("Built the Claude Desktop extension"));
}

/// Homebrew's `/opt/homebrew/bin/crystalline` is a link into the Cellar.
/// The link is pinned, never its target, so an upgrade does not break it.
#[cfg(unix)]
#[test]
fn a_linked_crystalline_on_path_is_pinned_as_the_link() {
    let s = Setup::new();
    let cellar = s.home.path().join("Cellar/crystalline/0.31.0/bin");
    fake_binary(&cellar);
    let linked = s.home.path().join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(cellar.join(BINARY_NAME), linked.join(BINARY_NAME)).unwrap();
    let assert = s
        .install(&std::env::join_paths([&linked]).unwrap())
        .assert()
        .success();
    let link = linked.join(BINARY_NAME).display().to_string();
    assert_eq!(pinned(&s.bundle()), link, "the path is not resolved");
    assert_eq!(s.record()["extension"]["binary"], link);
    assert!(!stdout(&assert).contains("versioned folder"));
}

/// The extension is built even when its record cannot be saved: the report
/// still says where it is and how to install it, and names what is lost.
#[test]
fn a_record_that_cannot_be_saved_still_reports_the_build() {
    let s = Setup::new();
    // A folder where the record's partial file goes makes the save fail.
    let blocker = s.state().join("claude-desktop.json.partial");
    std::fs::create_dir_all(blocker.join("in-the-way")).unwrap();
    let assert = s.install(&s.path_env()).assert().success();
    let out = stdout(&assert);
    assert!(s.bundle().is_file());
    assert!(!s.record_file().exists());
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("Built the Claude Desktop extension"),
        "{out}"
    );
    assert!(
        lines[2].starts_with("To install it, open Claude Desktop"),
        "{out}"
    );
    let notice = format!(
        "Crystalline could not remember this build in {} (",
        s.record_file().display()
    );
    assert!(lines[3].starts_with(&notice), "{out}");
    assert!(
        lines[3].ends_with("), so doctor and uninstall will not know about it."),
        "{out}"
    );
    assert_eq!(lines[4], FOOTER);

    let assert = s.install(&s.path_env()).arg("--json").assert().success();
    let report: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(
        report["extension"]["file"],
        s.bundle().display().to_string()
    );
    let notices = report["notices"].as_array().unwrap();
    assert_eq!(notices.len(), 1, "{report}");
    assert!(
        notices[0].as_str().unwrap().starts_with(&notice),
        "{report}"
    );
}

/// A record this crystalline cannot read (bad JSON, or a newer format) is
/// written fresh, and the report says so.
#[test]
fn an_unreadable_record_is_written_fresh_and_said_so() {
    for (content, reason) in [
        ("not json", "it is not valid JSON: "),
        (
            r#"{"format": 2}"#,
            "it has record format 2, which this crystalline does not know",
        ),
    ] {
        let s = Setup::new();
        std::fs::create_dir_all(s.state()).unwrap();
        std::fs::write(s.record_file(), content).unwrap();
        let assert = s.install(&s.path_env()).assert().success();
        let out = stdout(&assert);
        let start = format!("{} could not be read ({reason}", s.record_file().display());
        let line = out
            .lines()
            .find(|l| l.starts_with(&start))
            .unwrap_or_else(|| panic!("{content}: {out}"));
        assert!(line.ends_with("), so it was written fresh."), "{line}");
        assert_eq!(s.record()["format"], 1);
        assert_eq!(s.record()["extension"]["opened"], false);
        // The next run reads it again and says nothing more.
        let again = stdout(&s.install(&s.path_env()).assert().success());
        assert!(!again.contains("could not be read"), "{again}");
    }
}

const NO_OPEN_LINE: &str = "To install it, open Claude Desktop, go to Settings > Extensions > Advanced settings > Install Extension... and pick this file.";

const OPENED_LINE: &str = "Claude Desktop shows its install dialog. Confirm it there.";

#[test]
fn the_extension_is_opened_in_claude_desktop() {
    let s = Setup::new();
    let seam = format!("log:{}", s.open_log().display());
    let assert = s.install_opening(&s.path_env(), &seam).assert().success();
    assert_eq!(
        std::fs::read_to_string(s.open_log()).unwrap(),
        format!("open {}\n", s.bundle().display())
    );
    assert_eq!(s.record()["extension"]["opened"], true);
    assert_eq!(
        s.record()["offered"],
        env!("CARGO_PKG_VERSION"),
        "an opened extension counts as offered, so a bridge of this version does not offer it again"
    );
    let out = stdout(&assert);
    assert!(out.contains(OPENED_LINE), "{out}");
    assert!(!out.contains(NO_OPEN_LINE), "{out}");
    assert!(!out.contains("Could not open"), "{out}");

    let assert = s
        .install_opening(&s.path_env(), &seam)
        .arg("--json")
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(report["extension"]["opened"], true);
    assert!(report["extension"]["open_error"].is_null(), "{report}");
}

#[test]
fn a_failed_open_prints_the_path_and_the_steps() {
    let s = Setup::new();
    // The isolation environment's own value.
    let assert = s.install_opening(&s.path_env(), "fail").assert().success();
    assert!(s.bundle().is_file());
    assert_eq!(s.record()["extension"]["opened"], false);
    assert!(
        s.record()["offered"].is_null(),
        "a failed open offered nothing"
    );
    let out = stdout(&assert);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[2], "Could not open Claude Desktop: the test seam refused it.",
        "{out}"
    );
    assert_eq!(lines[3], NO_OPEN_LINE, "{out}");
    let explorer = "You can also double-click the file in File Explorer.";
    if cfg!(windows) {
        assert_eq!(lines[4], explorer, "{out}");
    } else {
        assert!(!out.contains(explorer), "{out}");
    }
    assert!(!out.contains(OPENED_LINE), "{out}");
    assert_eq!(lines[lines.len() - 1], FOOTER);

    let assert = s
        .install_opening(&s.path_env(), "fail")
        .arg("--json")
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(report["extension"]["opened"], false);
    assert_eq!(
        report["extension"]["open_error"],
        "the test seam refused it"
    );
}

#[test]
fn the_isolation_environment_refuses_the_open() {
    let s = Setup::new();
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, s.home.path());
    let assert = cmd
        .env("PATH", s.path_env())
        .args(["install", "claude-desktop", "--skip-skills"])
        .assert()
        .success();
    assert!(
        stdout(&assert).contains("Could not open Claude Desktop: the test seam refused it."),
        "the seam is set to fail unless a test says otherwise"
    );
}

#[test]
fn no_open_never_calls_the_opener() {
    let s = Setup::new();
    let seam = format!("log:{}", s.open_log().display());
    let assert = s
        .install_opening(&s.path_env(), &seam)
        .arg("--no-open")
        .assert()
        .success();
    assert!(!s.open_log().exists());
    assert_eq!(s.record()["extension"]["opened"], false);
    let out = stdout(&assert);
    assert!(out.contains(NO_OPEN_LINE), "{out}");
    assert!(!out.contains("Could not open"), "{out}");
}

// --- the crystalline-intelligence skill ---------------------------------

const SKILL_ROUTE_LINE: &str =
    "Upload it in Claude Desktop under Customize > Skills and turn it on. Do not unpack the zip.";

const OLD_SKILL_LINE: &str = "If you uploaded the older crystalline-memory skill, delete it there.";

/// The shipped skill's `SKILL.md`, as the zip must carry it.
fn skill_text() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/crystalline-intelligence/SKILL.md"),
    )
    .unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Setup {
    /// The person's Downloads folder under the temp home: `HOME` and
    /// `USERPROFILE` both point at it in the isolation environment.
    fn downloads(&self) -> PathBuf {
        self.home.path().join("Downloads")
    }

    /// `crystalline install claude-desktop` with the skill step, opening and
    /// revealing through the logging seam.
    fn install_with_skill(&self) -> Command {
        let mut cmd = crate::common::crystalline();
        crate::common::isolate(&mut cmd, self.home.path());
        cmd.env("PATH", self.path_env())
            .env(
                "CRYSTALLINE_TEST_DESKTOP_OPEN",
                format!("log:{}", self.open_log().display()),
            )
            .args(["install", "claude-desktop"]);
        cmd
    }

    fn open_lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.open_log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn zips(&self, dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".zip"))
            .collect();
        names.sort();
        names
    }
}

#[test]
fn the_skill_is_saved_to_downloads_and_revealed() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    let assert = s.install_with_skill().assert().success();
    let zip = s.downloads().join("crystalline-intelligence.zip");
    assert_eq!(
        String::from_utf8(entry(&zip, "crystalline-intelligence/SKILL.md")).unwrap(),
        skill_text()
    );
    assert_eq!(
        s.open_lines(),
        vec![
            format!("open {}", s.bundle().display()),
            format!("reveal {}", zip.display()),
        ]
    );
    let skill = &s.record()["skill"];
    assert_eq!(skill["name"], "crystalline-intelligence");
    assert_eq!(skill["sha256"], sha256(skill_text().as_bytes()));
    assert_eq!(skill["file"], zip.display().to_string());
    chrono::DateTime::parse_from_rfc3339(skill["saved_at"].as_str().unwrap())
        .expect("saved_at is RFC 3339");

    let out = stdout(&assert);
    let lines: Vec<&str> = out.lines().collect();
    let saved = format!(
        "Saved the crystalline-intelligence skill: {}",
        zip.display()
    );
    let at = lines
        .iter()
        .position(|l| *l == saved)
        .unwrap_or_else(|| panic!("{out}"));
    assert_eq!(lines[at + 1], SKILL_ROUTE_LINE, "{out}");
    assert_eq!(lines[at + 2], OLD_SKILL_LINE, "{out}");
    assert!(
        lines[..at].contains(&OPENED_LINE),
        "the extension lines come first:\n{out}"
    );
    assert_eq!(lines[lines.len() - 1], FOOTER);
}

#[test]
fn an_existing_zip_is_never_overwritten() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    let planted = s.downloads().join("crystalline-intelligence.zip");
    std::fs::write(&planted, b"someone else's zip").unwrap();
    let assert = s.install_with_skill().assert().success();
    assert_eq!(std::fs::read(&planted).unwrap(), b"someone else's zip");
    let zip = s.downloads().join("crystalline-intelligence 1.zip");
    assert_eq!(
        String::from_utf8(entry(&zip, "crystalline-intelligence/SKILL.md")).unwrap(),
        skill_text()
    );
    assert_eq!(s.record()["skill"]["file"], zip.display().to_string());
    assert!(
        stdout(&assert).contains(&format!(
            "Saved the crystalline-intelligence skill: {}",
            zip.display()
        )),
        "{}",
        stdout(&assert)
    );
}

#[test]
fn an_unchanged_skill_is_not_saved_twice() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    s.install_with_skill().assert().success();
    let zip = s.downloads().join("crystalline-intelligence.zip");
    let first = s.record()["skill"].clone();
    std::fs::remove_file(s.open_log()).unwrap();

    let assert = s.install_with_skill().assert().success();
    assert_eq!(s.zips(&s.downloads()), vec!["crystalline-intelligence.zip"]);
    assert_eq!(
        s.open_lines(),
        vec![format!("open {}", s.bundle().display())],
        "the extension is opened again, the zip is not shown again"
    );
    assert_eq!(
        s.record()["skill"],
        first,
        "the record keeps the first save"
    );
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "The crystalline-intelligence skill has not changed since you saved it: {}\n",
            zip.display()
        )),
        "{out}"
    );
    assert!(!out.contains("Saved the"), "{out}");
    assert!(!out.contains(SKILL_ROUTE_LINE), "{out}");

    let assert = s.install_with_skill().arg("--json").assert().success();
    let report: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(report["skill"]["status"], "unchanged", "{report}");
    assert_eq!(report["skill"]["file"], zip.display().to_string());
    assert_eq!(report["skill"]["name"], "crystalline-intelligence");
}

/// A recorded zip that is gone is saved again, and shown again.
#[test]
fn a_deleted_zip_is_saved_again() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    s.install_with_skill().assert().success();
    let zip = s.downloads().join("crystalline-intelligence.zip");
    std::fs::remove_file(&zip).unwrap();
    std::fs::remove_file(s.open_log()).unwrap();
    let assert = s.install_with_skill().arg("--json").assert().success();
    assert!(zip.is_file());
    assert_eq!(s.open_lines()[1], format!("reveal {}", zip.display()));
    let report: serde_json::Value = serde_json::from_str(stdout(&assert).trim()).unwrap();
    assert_eq!(report["skill"]["status"], "saved", "{report}");
    assert_eq!(report["skill"]["revealed"], true, "{report}");
}

#[test]
fn without_a_downloads_folder_the_zip_goes_to_the_state_folder() {
    let s = Setup::new();
    let assert = s.install_with_skill().assert().success();
    assert!(!s.downloads().exists(), "no Downloads folder is made");
    let zip = s
        .state()
        .join("claude-desktop")
        .join("crystalline-intelligence.zip");
    assert_eq!(
        String::from_utf8(entry(&zip, "crystalline-intelligence/SKILL.md")).unwrap(),
        skill_text()
    );
    assert_eq!(s.record()["skill"]["file"], zip.display().to_string());
    let out = stdout(&assert);
    assert!(
        out.contains(&format!(
            "Saved the crystalline-intelligence skill: {}\n",
            zip.display()
        )),
        "{out}"
    );
}

#[test]
fn skip_skills_leaves_the_skill_alone() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    let assert = s
        .install_with_skill()
        .arg("--skip-skills")
        .assert()
        .success();
    assert!(s.zips(&s.downloads()).is_empty());
    assert!(s.zips(s.bundle().parent().unwrap()).is_empty());
    let record = s.record();
    assert!(record.get("skill").is_none_or(|v| v.is_null()), "{record}");
    assert!(
        !s.open_lines().iter().any(|l| l.starts_with("reveal ")),
        "{:?}",
        s.open_lines()
    );
    assert!(!stdout(&assert).contains("skill"), "{}", stdout(&assert));
}

/// A refused reveal is one notice; the zip is saved and the run succeeds.
#[test]
fn a_failed_reveal_is_one_notice() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    let assert = s
        .install_with_skill()
        .env("CRYSTALLINE_TEST_DESKTOP_OPEN", "fail")
        .assert()
        .success();
    assert!(s.downloads().join("crystalline-intelligence.zip").is_file());
    let out = stdout(&assert);
    let notice = "Could not show the file: the test seam refused it.";
    assert_eq!(out.matches(notice).count(), 1, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[lines.len() - 2], notice, "{out}");
    assert_eq!(lines[lines.len() - 1], FOOTER);
}

/// `--no-open` hands nothing to Claude Desktop and shows nothing in the file
/// manager: the zip is saved and its path printed.
#[test]
fn no_open_saves_the_skill_without_showing_it() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    let assert = s.install_with_skill().arg("--no-open").assert().success();
    assert!(s.downloads().join("crystalline-intelligence.zip").is_file());
    assert!(!s.open_log().exists(), "{:?}", s.open_lines());
    assert!(stdout(&assert).contains(SKILL_ROUTE_LINE));
}

/// `--skip-mcp` saves the skill alone and remembers it.
#[test]
fn skip_mcp_saves_only_the_skill() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    let assert = s.install_with_skill().arg("--skip-mcp").assert().success();
    assert!(!s.bundle().exists());
    let zip = s.downloads().join("crystalline-intelligence.zip");
    assert!(zip.is_file());
    assert_eq!(s.open_lines(), vec![format!("reveal {}", zip.display())]);
    assert!(s.record()["extension"].is_null(), "{}", s.record());
    assert_eq!(s.record()["skill"]["file"], zip.display().to_string());
    let out = stdout(&assert);
    assert!(
        out.starts_with("Saved the crystalline-intelligence skill: "),
        "{out}"
    );
}

/// A skill that cannot be saved anywhere fails a run that had nothing else
/// to do.
#[test]
fn a_skill_that_cannot_be_saved_fails_a_skill_only_run() {
    let s = Setup::new();
    // No Downloads folder, and a file where the state folder's
    // claude-desktop folder goes.
    std::fs::create_dir_all(s.state()).unwrap();
    std::fs::write(s.state().join("claude-desktop"), "in the way").unwrap();
    let assert = s.install_with_skill().arg("--skip-mcp").assert().failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("Could not save the crystalline-intelligence skill: "),
        "{stderr}"
    );
    assert!(!s.open_log().exists());
}

const NO_RECORD_LINE: &str =
    "Crystalline has no record of a Claude Desktop install on this computer.";
const REMOVE_LINE: &str = "Crystalline cannot remove the extension from Claude Desktop. Remove it there: Settings > Extensions, then Crystalline.";
const SKILL_STAYS_LINE: &str =
    "The skill stays in Claude Desktop until you delete it under Customize > Skills.";

impl Setup {
    fn uninstall(&self) -> Command {
        let mut cmd = crate::common::crystalline();
        crate::common::isolate(&mut cmd, self.home.path());
        cmd.env("PATH", self.path_env())
            .args(["uninstall", "claude-desktop"]);
        cmd
    }
}

#[test]
fn uninstall_removes_the_file_and_the_record_and_says_where_to_finish() {
    let s = Setup::new();
    std::fs::create_dir_all(s.downloads()).unwrap();
    s.install_with_skill().assert().success();
    let zip = s.downloads().join("crystalline-intelligence.zip");
    assert!(zip.is_file());
    let bundle = s.bundle();
    let mut previous = bundle.as_os_str().to_owned();
    previous.push(".previous");
    let mut partial = bundle.as_os_str().to_owned();
    partial.push(".partial");
    std::fs::write(&previous, "old").unwrap();
    std::fs::write(&partial, "half").unwrap();
    let lock = s.state().join("claude-desktop.offer.lock");
    std::fs::write(&lock, "").unwrap();
    let record_partial = s.state().join("claude-desktop.json.partial");
    std::fs::write(&record_partial, "{").unwrap();

    let assert = s.uninstall().assert().success();
    let out = stdout(&assert);
    assert_eq!(
        out,
        format!(
            "Removed the built extension ({}) and Crystalline's record of it.\n{REMOVE_LINE}\n{SKILL_STAYS_LINE}\n",
            bundle.display()
        )
    );
    assert!(!bundle.exists());
    assert!(!bundle.parent().unwrap().exists(), "empty folder is gone");
    assert!(!s.record_file().exists());
    assert!(!Path::new(&previous).exists());
    assert!(!Path::new(&partial).exists());
    assert!(!lock.exists());
    assert!(!record_partial.exists());
    assert!(zip.is_file(), "the skill zip stays");
}

#[test]
fn uninstall_keeps_a_folder_that_is_not_empty() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let other = s.bundle().parent().unwrap().join("other.txt");
    std::fs::write(&other, "mine").unwrap();
    s.uninstall().assert().success();
    assert!(!s.bundle().exists());
    assert!(other.is_file());
}

#[test]
fn uninstall_without_a_record_says_so() {
    let s = Setup::new();
    let assert = s.uninstall().assert().success();
    assert_eq!(
        stdout(&assert),
        format!("{NO_RECORD_LINE}\n{REMOVE_LINE}\n{SKILL_STAYS_LINE}\n")
    );
}

#[test]
fn uninstall_json_reports_what_was_removed() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let assert = s.uninstall().arg("--json").assert().success();
    let v: serde_json::Value = serde_json::from_str(&stdout(&assert)).unwrap();
    assert_eq!(v["target"], "claude-desktop");
    assert_eq!(v["removed_file"], s.bundle().display().to_string());
    assert_eq!(v["removed_record"], true);
    let assert = s.uninstall().arg("--json").assert().success();
    let v: serde_json::Value = serde_json::from_str(&stdout(&assert)).unwrap();
    assert!(v["removed_file"].is_null());
    assert_eq!(v["removed_record"], false);
}

#[test]
fn uninstall_refuses_project_and_force() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let cases: [(&str, &str); 2] = [
        (
            "--project",
            "--project does not apply to claude-desktop: Claude Desktop extensions are per user.",
        ),
        ("--force", "--force does not apply to claude-desktop."),
    ];
    for (flag, error) in cases {
        let assert = s.uninstall().arg(flag).assert().failure();
        let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
        assert!(stderr.contains(error), "{flag}: {stderr}");
        assert!(s.bundle().is_file(), "{flag} removed nothing");
        assert!(s.record_file().is_file(), "{flag} removed nothing");
    }
}

#[test]
fn uninstall_leaves_the_install_receipt_alone() {
    let s = Setup::new();
    s.install(&s.path_env()).assert().success();
    let receipt = s.state().join("installs.json");
    let planted = b"{\"installs\": [ ]}\n".to_vec();
    std::fs::write(&receipt, &planted).unwrap();
    s.uninstall().assert().success();
    assert_eq!(std::fs::read(&receipt).unwrap(), planted);
}
