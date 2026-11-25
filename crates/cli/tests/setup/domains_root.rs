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
