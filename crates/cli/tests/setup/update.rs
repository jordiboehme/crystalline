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

/// `update verify-sums` with the minisign CLI test key trusted through the
/// debug-only seam.
#[cfg(debug_assertions)]
fn verify_sums_with_the_test_key(tag: &str) -> std::process::Output {
    crate::common::crystalline()
        .env("CRYSTALLINE_TEST_UPDATE_KEY", fixture("cli-test.pub"))
        .args(["update", "verify-sums", "--tag", tag])
        .arg(fixture("v0.0.0-SHA256SUMS"))
        .arg(fixture("v0.0.0-SHA256SUMS.cli-test-key.minisig"))
        .output()
        .unwrap()
}

#[cfg(debug_assertions)]
#[test]
fn verify_sums_accepts_a_signature_by_a_trusted_key() {
    let out = verify_sums_with_the_test_key("v0.0.0");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "verified SHA256SUMS for v0.0.0\n"
    );
    assert!(stderr.is_empty(), "nothing on stderr: {stderr}");
}

#[cfg(debug_assertions)]
#[test]
fn verify_sums_refuses_a_signature_for_another_tag() {
    let out = verify_sums_with_the_test_key("v0.0.1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert_eq!(
        stderr.trim(),
        "the signature over SHA256SUMS is for \"tag:v0.0.0\", not for tag v0.0.1"
    );
    assert!(out.stdout.is_empty(), "nothing on stdout");
}

/// A release build has no key seam: the environment cannot add a trusted key.
#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_ignores_the_test_key() {
    let out = crate::common::crystalline()
        .env("CRYSTALLINE_TEST_UPDATE_KEY", fixture("cli-test.pub"))
        .args(["update", "verify-sums", "--tag", "v0.0.0"])
        .arg(fixture("v0.0.0-SHA256SUMS"))
        .arg(fixture("v0.0.0-SHA256SUMS.cli-test-key.minisig"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "SHA256SUMS is signed by a key this build does not trust"
    );
}

/// S11: the key seam is read in exactly one place, an item compiled only
/// into debug builds. It lives in core (`update::trusted_keys`), which both
/// this CLI and the service's update client use, so no crate reads it twice.
#[test]
fn the_test_key_seam_is_debug_only() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let name = "\"CRYSTALLINE_TEST_UPDATE_KEY\"";
    let mut reads = Vec::new();
    for krate in ["cli", "core", "service"] {
        // Every .rs file under src, subfolders included.
        let mut files = Vec::new();
        let mut dirs = vec![crates.join(krate).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (at, line) in lines.iter().enumerate() {
                if !line.contains(name) {
                    continue;
                }
                reads.push(format!("{krate}: {}", path.display()));
                // The read sits in a function whose attribute is the cfg.
                let item = lines[..at]
                    .iter()
                    .rposition(|l| l.starts_with("fn ") || l.starts_with("pub"))
                    .expect("the read sits in an item");
                assert_eq!(
                    lines[item - 1].trim(),
                    "#[cfg(debug_assertions)]",
                    "{}:{}: the read of {name} is not debug-only",
                    path.display(),
                    at + 1
                );
            }
        }
    }
    assert_eq!(
        reads.len(),
        1,
        "the seam is read in exactly one place: {reads:?}"
    );
    assert!(reads[0].starts_with("core: "), "{reads:?}");
}
