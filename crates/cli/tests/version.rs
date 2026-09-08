//! `crystalline --version` / `-V`: names the copyright, the license and the
//! source, per AGPL section 13 (a network-served copy has to offer its users
//! the source, so the link belongs where a user of a running instance can
//! see it). Three lines, no blank line, both flags byte-identical.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

#[test]
fn long_and_short_version_flags_print_the_same_bytes() {
    let long = bin().arg("--version").output().unwrap();
    let short = bin().arg("-V").output().unwrap();
    assert!(long.status.success());
    assert!(short.status.success());
    assert_eq!(
        long.stdout, short.stdout,
        "-V and --version must be byte-identical"
    );
}

#[test]
fn version_output_names_the_copyright_the_license_and_the_source() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "exactly three lines, no blank line: {stdout:?}"
    );
    assert_eq!(
        lines[0],
        format!("crystalline {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        lines[1].contains("Copyright (C) 2026 Jordi Boehme"),
        "second line names the copyright holder: {}",
        lines[1]
    );
    assert!(
        lines[1].contains(env!("CARGO_PKG_LICENSE")),
        "second line names the license: {}",
        lines[1]
    );
    assert_eq!(lines[2], env!("CARGO_PKG_REPOSITORY"));
}
