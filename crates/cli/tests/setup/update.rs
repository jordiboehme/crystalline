//! `crystalline update verify-sums`: the publish job's proof that the
//! release key pair works. Hidden, and it trusts only the compiled-in keys.

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../core/tests/fixtures/update")
        .join(name)
}

#[test]
fn verify_sums_refuses_a_test_signature() {
    let sums = fixture("v0.0.0-SHA256SUMS");
    let sig = fixture("v0.0.0-SHA256SUMS.test-key.minisig");
    assert!(sums.is_file() && sig.is_file(), "the fixtures exist");
    let out = crate::common::crystalline()
        .args(["update", "verify-sums", "--tag", "v0.0.0"])
        .arg(&sums)
        .arg(&sig)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr.trim(),
        "SHA256SUMS is signed by a key this build does not trust"
    );
    assert!(out.stdout.is_empty(), "nothing on stdout");
}

#[test]
fn verify_sums_is_hidden() {
    for args in [&["--help"][..], &["update", "--help"][..]] {
        let out = crate::common::crystalline().args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}");
        let help = String::from_utf8_lossy(&out.stdout);
        assert!(!help.contains("verify-sums"), "{args:?} names it: {help}");
    }
}
