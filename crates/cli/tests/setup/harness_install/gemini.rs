//! `crystalline install gemini` and `crystalline uninstall gemini`.

use super::support::*;

#[test]
fn a_harness_that_is_not_ready_is_refused_and_writes_nothing() {
    let b = sandbox();
    let out = cmd(&b)
        .args(["install", "gemini"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("crystalline install gemini is not available in this build yet"),
        "{stderr}"
    );
    assert!(!b.home.join(".gemini").exists(), "nothing written");
    assert!(
        !crate::common::isolated_state_dir(&b.home)
            .join("installs.json")
            .exists()
    );
    let out = cmd(&b)
        .args(["uninstall", "gemini"])
        .assert()
        .failure()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("crystalline install gemini is not available in this build yet"),
        "{stderr}"
    );
}
