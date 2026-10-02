//! `crystalline install qwen` and `crystalline uninstall qwen`.

use super::support::*;

#[test]
fn a_harness_that_is_not_ready_is_refused_and_writes_nothing() {
    let b = sandbox();
    let out = cmd(&b)
        .args(["install", "qwen"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("crystalline install qwen is not available in this build yet"),
        "{stderr}"
    );
    assert!(!b.home.join(".qwen").exists(), "nothing written");
    assert!(
        !crate::common::isolated_state_dir(&b.home)
            .join("installs.json")
            .exists()
    );
    cmd(&b).args(["uninstall", "qwen"]).assert().failure();
}
