//! `crystalline domain add NAME` without a path, registered in the CLI's own
//! process: on a configuration from before 0.37.0 (a domain in
//! `~/Documents/Crystalline`, no `domains_root`) it looks beside the old
//! domains and saves the pin with the new entry; on an empty one it looks
//! under the hidden `~/.crystalline`. Every child runs in a scratch home.

use std::path::{Path, PathBuf};

use crystalline_core::config::root_pin::{PinRecord, config_key};
use crystalline_core::config::{self, DomainEntry, GlobalConfig};

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_SERVICE_HTTP", "false")
        .env("CRYSTALLINE_TEST_DAEMON_TASK", "ready")
        .args(args)
        .output()
        .unwrap()
}

fn manifest_at(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- {name}\n\n## When to Use\n\n- {name}\n"
        ),
    )
    .unwrap();
}

fn config_file(home: &Path) -> PathBuf {
    home.join("work").join("config.yaml")
}

#[test]
fn a_name_without_a_path_lands_beside_the_old_domains_and_pins() {
    let home = tempfile::tempdir().unwrap();
    let docs = home.path().join("Documents").join("Crystalline");
    manifest_at(&docs.join("old"), "old");
    manifest_at(&docs.join("new"), "new");
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("old".into(), DomainEntry::file(docs.join("old")));
    let config_path = config_file(home.path());
    config::save_yaml(&config_path, &cfg).unwrap();

    let out = run(
        home.path(),
        &[
            "domain",
            "add",
            "new",
            "--config",
            config_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let saved: GlobalConfig = config::load_yaml(&config_path).unwrap();
    assert_eq!(
        saved.domains_root.as_deref(),
        Some(Path::new("~/Documents/Crystalline"))
    );
    let entry = saved.domains.get("new").expect("registered");
    assert_eq!(
        std::fs::canonicalize(entry.file_path().unwrap()).unwrap(),
        std::fs::canonicalize(docs.join("new")).unwrap()
    );
    let state = crate::common::isolated_state_dir(home.path());
    let (record, problem) = PinRecord::load(&state);
    assert!(problem.is_none(), "{problem:?}");
    let key = config_key(Some(&config_path));
    assert!(
        record.checked.iter().any(|c| c.config == key && c.pinned),
        "{record:?}"
    );
}

#[test]
fn a_name_without_a_path_looks_under_the_hidden_root_on_a_fresh_install() {
    let home = tempfile::tempdir().unwrap();
    let config_path = config_file(home.path());
    config::save_yaml(&config_path, &GlobalConfig::default()).unwrap();

    let out = run(
        home.path(),
        &[
            "domain",
            "add",
            "fresh",
            "--config",
            config_path.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    let expected = home.path().join(".crystalline").join("fresh");
    assert!(
        err.contains(&expected.display().to_string()),
        "names {}: {err}",
        expected.display()
    );
    let saved: GlobalConfig = config::load_yaml(&config_path).unwrap();
    assert!(saved.domains_root.is_none());
}

fn doctor(home: &Path, config_path: &Path, json: bool) -> String {
    let db = home.join("index.db");
    let mut args = vec![];
    if json {
        args.push("--json");
    }
    args.extend([
        "doctor",
        "--config",
        config_path.to_str().unwrap(),
        "--db",
        db.to_str().unwrap(),
    ]);
    let out = run(home, &args);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn doctor_shows_where_new_domains_go_and_why() {
    // A fresh install: the hidden default.
    let home = tempfile::tempdir().unwrap();
    let config_path = config_file(home.path());
    config::save_yaml(&config_path, &GlobalConfig::default()).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&doctor(home.path(), &config_path, true)).unwrap();
    let hidden = home.path().join(".crystalline");
    assert_eq!(
        report["domains_root"],
        serde_json::json!({
            "path": hidden.display().to_string(),
            "source": "default",
            "protected": false,
        })
    );
    let text = doctor(home.path(), &config_path, false);
    assert!(
        text.contains(&format!("domains root: {} (default)", hidden.display())),
        "{text}"
    );
    assert!(!hidden.exists(), "doctor creates nothing");

    // Before 0.37.0's first start: the pin is announced, and doctor writes
    // nothing.
    let docs = home.path().join("Documents").join("Crystalline");
    manifest_at(&docs.join("old"), "old");
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("old".into(), DomainEntry::file(docs.join("old")));
    config::save_yaml(&config_path, &cfg).unwrap();
    let before = std::fs::read(&config_path).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&doctor(home.path(), &config_path, true)).unwrap();
    assert_eq!(report["domains_root"]["pin_pending"], true, "{report}");
    let text = doctor(home.path(), &config_path, false);
    assert!(
        text.contains("domains_root will be set to ~/Documents/Crystalline at the next start"),
        "{text}"
    );
    assert_eq!(std::fs::read(&config_path).unwrap(), before);

    // Pinned: the source is the configuration, and on macOS the note says
    // how to send new domains to the hidden folder.
    cfg.domains_root = Some(PathBuf::from("~/Documents/Crystalline"));
    config::save_yaml(&config_path, &cfg).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&doctor(home.path(), &config_path, true)).unwrap();
    assert_eq!(report["domains_root"]["source"], "config");
    assert_eq!(
        report["domains_root"]["protected"],
        cfg!(target_os = "macos")
    );
    assert!(report["domains_root"].get("pin_pending").is_none());
    let text = doctor(home.path(), &config_path, false);
    let note = format!(
        "New domains are created under {}, which macOS privacy protection guards.",
        home.path().join("Documents/Crystalline").display()
    );
    assert_eq!(text.contains(&note), cfg!(target_os = "macos"), "{text}");

    // The environment wins and says so.
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home.path());
    let out = cmd
        .env("CRYSTALLINE_DOMAINS_ROOT", home.path().join("elsewhere"))
        .args(["--json", "doctor", "--config"])
        .arg(&config_path)
        .arg("--db")
        .arg(home.path().join("index.db"))
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["domains_root"]["source"], "environment");
}

/// A `config.yaml` that is a relative symbolic link into a dotfiles folder,
/// kept owner only, holding `cfg`. Answers the link's target.
#[cfg(unix)]
fn linked_config(home: &Path, cfg: &GlobalConfig) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let real = home.join("dotfiles").join("crystalline.yaml");
    config::save_yaml(&real, cfg).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = config_file(home);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("../dotfiles/crystalline.yaml", &link).unwrap();
    real
}

#[cfg(unix)]
fn assert_still_linked(home: &Path, real: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let link = config_file(home);
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link stays a link"
    );
    let mode = std::fs::metadata(real).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    for (dir, only) in [
        (link.parent().unwrap(), "config.yaml"),
        (real.parent().unwrap(), "crystalline.yaml"),
    ] {
        let names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![only.to_string()], "no temporary file left");
    }
}

#[cfg(unix)]
#[test]
fn a_registration_in_the_cli_pins_through_a_linked_config() {
    let home = tempfile::tempdir().unwrap();
    let docs = home.path().join("Documents").join("Crystalline");
    manifest_at(&docs.join("old"), "old");
    manifest_at(&docs.join("new"), "new");
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("old".into(), DomainEntry::file(docs.join("old")));
    let real = linked_config(home.path(), &cfg);
    let link = config_file(home.path());

    let out = run(
        home.path(),
        &["domain", "add", "new", "--config", link.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_still_linked(home.path(), &real);
    let saved: GlobalConfig = config::load_yaml(&real).unwrap();
    assert_eq!(
        saved.domains_root.as_deref(),
        Some(Path::new("~/Documents/Crystalline"))
    );
    assert!(saved.domains.contains_key("new"));
}

#[cfg(unix)]
#[test]
fn a_config_set_in_the_cli_writes_through_a_linked_config() {
    let home = tempfile::tempdir().unwrap();
    let real = linked_config(home.path(), &GlobalConfig::default());
    let link = config_file(home.path());
    let out = run(
        home.path(),
        &[
            "config",
            "set",
            "domains_root",
            "~/kb",
            "--config",
            link.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_still_linked(home.path(), &real);
    let saved: GlobalConfig = config::load_yaml(&real).unwrap();
    assert_eq!(saved.domains_root.as_deref(), Some(Path::new("~/kb")));
}

/// A read-only instance never pins at its start, so doctor does not promise
/// it. `crystalline config set` is refused on it as well (file mode), so
/// doctor names the file to edit, and the edit is all it takes.
#[test]
fn doctor_on_a_read_only_instance_names_the_file_to_edit() {
    let home = tempfile::tempdir().unwrap();
    let docs = home.path().join("Documents").join("Crystalline");
    manifest_at(&docs.join("old"), "old");
    let mut cfg = GlobalConfig {
        service: Some(config::ServiceConfig {
            read_only: Some(true),
            ..config::ServiceConfig::default()
        }),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert("old".into(), DomainEntry::file(docs.join("old")));
    let config_path = config_file(home.path());
    config::save_yaml(&config_path, &cfg).unwrap();

    let report: serde_json::Value =
        serde_json::from_str(&doctor(home.path(), &config_path, true)).unwrap();
    assert_eq!(report["domains_root"]["pin_pending"], true, "{report}");
    assert_eq!(report["domains_root"]["read_only"], true, "{report}");
    let text = doctor(home.path(), &config_path, false);
    assert!(!text.contains("at the next start"), "{text}");
    assert!(
        text.contains(&format!(
            "add `domains_root: ~/Documents/Crystalline` to {}",
            config_path.display()
        )),
        "{text}"
    );
    assert!(
        !text.contains("crystalline config set domains_root ~/Documents"),
        "{text}"
    );

    // Why doctor does not name `config set`: it is refused here.
    let out = run(
        home.path(),
        &[
            "config",
            "set",
            "domains_root",
            "~/Documents/Crystalline",
            "--config",
            config_path.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());

    // The edit doctor names is all it takes: the note is gone.
    let mut text = std::fs::read_to_string(&config_path).unwrap();
    text.push_str("domains_root: ~/Documents/Crystalline\n");
    std::fs::write(&config_path, text).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&doctor(home.path(), &config_path, true)).unwrap();
    assert!(
        report["domains_root"].get("pin_pending").is_none(),
        "{report}"
    );
    assert_eq!(report["domains_root"]["source"], "config", "{report}");
}

/// The macOS note names the root new domains really go to.
#[cfg(target_os = "macos")]
#[test]
fn doctor_names_the_guarded_root_it_warns_about() {
    let home = tempfile::tempdir().unwrap();
    let config_path = config_file(home.path());
    let cfg = GlobalConfig {
        domains_root: Some(PathBuf::from("~/Desktop/kb")),
        ..GlobalConfig::default()
    };
    config::save_yaml(&config_path, &cfg).unwrap();
    let text = doctor(home.path(), &config_path, false);
    let root = home.path().join("Desktop").join("kb");
    assert!(
        text.contains(&format!(
            "New domains are created under {}, which macOS privacy protection guards.",
            root.display()
        )),
        "{text}"
    );
    assert!(!text.contains("~/Documents/Crystalline"), "{text}");
}
