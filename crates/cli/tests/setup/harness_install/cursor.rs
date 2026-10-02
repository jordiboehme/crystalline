//! `crystalline install cursor` and `crystalline uninstall cursor`.

use super::support::*;

#[test]
fn a_harness_that_is_not_ready_is_refused_and_writes_nothing() {
    let b = sandbox();
    let out = cmd(&b)
        .args(["install", "cursor"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("crystalline install cursor is not available in this build yet"),
        "{stderr}"
    );
    assert!(!b.home.join(".cursor").exists(), "nothing written");
    assert!(
        !crate::common::isolated_state_dir(&b.home)
            .join("installs.json")
            .exists()
    );
    cmd(&b).args(["uninstall", "cursor"]).assert().failure();
}
