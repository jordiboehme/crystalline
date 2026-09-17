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
        lines[1].contains("Copyright (C) 2026 Jordi Böhme"),
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

/// The CLI's `--version` block and the daemon's banner name the same copyright
/// holder and year. Two `concat!` literals in two crates is what the version
/// task's review flagged: `concat!` takes literal tokens only and cannot read a
/// const, so an assertion is the whole of what can hold them together. This is
/// the test that fails the day one of them is changed alone.
#[test]
fn the_version_block_and_the_banner_name_one_holder() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_crystalline"))
        .arg("--version")
        .output()
        .expect("the binary runs");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let second = stdout
        .lines()
        .nth(1)
        .expect("the version block has three lines");
    assert!(
        second.starts_with(crystalline_service::daemon::COPYRIGHT_HOLDER),
        "the CLI prints `{second}`, the daemon's banner says `{}`",
        crystalline_service::daemon::COPYRIGHT_HOLDER
    );
}
