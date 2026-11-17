//! The install marker each package ships beside the binary, and the
//! official-build flag, read from the packaging sources: the packages are
//! only built by the release workflow, and this keeps their sources in
//! step with what `crystalline_core::update::detect` reads on every
//! platform.
//!
//! Detection resolves each marker from the canonical exe's folder: the
//! .deb and Docker marker at `../lib/crystalline/install-source`, the
//! Homebrew one at `../libexec/install-source` and the MSI one beside the
//! exe (pinned in `wix.rs`).

use std::path::PathBuf;

use crystalline_core::update::MARKER_FILE;

/// A file of the repository, by its path from the root.
fn repo_file(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read {}: {e}", full.display()))
}

/// A marker file's bytes, `None` when it is missing.
fn marker(path: &str) -> Option<String> {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    std::fs::read_to_string(full).ok()
}

/// The .deb installs `usr/lib/crystalline/install-source`, saying `deb`,
/// next to the `usr/lib` its `usr/bin/crystalline` reads as
/// `../lib/crystalline`.
#[test]
fn the_deb_ships_the_marker() {
    let manifest: toml::Table = repo_file("crates/cli/Cargo.toml")
        .parse()
        .expect("crates/cli/Cargo.toml parses");
    let assets = manifest["package"]["metadata"]["deb"]["assets"]
        .as_array()
        .expect("[package.metadata.deb] has assets");
    let rows: Vec<Vec<&str>> = assets
        .iter()
        .map(|row| {
            row.as_array()
                .expect("an asset row is an array")
                .iter()
                .map(|cell| cell.as_str().expect("an asset cell is a string"))
                .collect()
        })
        .collect();
    assert!(
        rows.contains(&vec!["target/release/crystalline", "usr/bin/", "755"]),
        "the binary goes to usr/bin, which the marker path below assumes"
    );
    let source = format!("debian/{MARKER_FILE}");
    let target = format!("usr/lib/crystalline/{MARKER_FILE}");
    assert!(
        rows.contains(&vec![source.as_str(), target.as_str(), "644"]),
        "no [{source}, {target}, 644] row in {rows:?}"
    );
    assert_eq!(
        marker(&format!("crates/cli/{source}")).as_deref(),
        Some("deb\n")
    );
}

/// The Homebrew formula release.yml writes installs the binary with
/// `bin.install` and writes `libexec/install-source`, saying `homebrew`,
/// in the same `install`.
#[test]
fn the_formula_writes_the_marker() {
    let workflow: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&repo_file(".github/workflows/release.yml"))
            .expect("release.yml parses");
    let steps = workflow["jobs"]["homebrew"]["steps"]
        .as_sequence()
        .expect("the homebrew job has steps");
    let script = steps
        .iter()
        .filter_map(|step| step["run"].as_str())
        .find(|run| run.contains("<< FORMULA"))
        .expect("a homebrew step writes the formula");
    let body = script
        .split_once("<< FORMULA\n")
        .and_then(|(_, rest)| rest.split_once("\nFORMULA"))
        .map(|(body, _)| body)
        .expect("the formula heredoc closes");
    let install = body
        .split_once("def install\n")
        .and_then(|(_, rest)| rest.split_once("\n  end\n"))
        .map(|(install, _)| install)
        .expect("the formula has an install method");
    let lines: Vec<&str> = install.lines().map(str::trim).collect();
    assert_eq!(
        lines,
        [
            r#"bin.install "crystalline""#,
            &format!(r#"(libexec/"{MARKER_FILE}").write "homebrew\n""#),
        ],
        "the formula's install"
    );
}

/// The image copies `docker/install-source`, saying `docker`, to
/// `/usr/local/lib/crystalline/`, beside the `/usr/local/bin` the binary
/// goes to: in the `runtime` stage (so the model variant inherits it),
/// owned by root (no `--chown`) and read-only, with no `RUN` step.
#[test]
fn the_dockerfile_ships_the_marker() {
    let dockerfile = repo_file("Dockerfile");
    let runtime = dockerfile
        .split_once(" AS runtime\n")
        .and_then(|(_, rest)| rest.split("\nFROM ").next())
        .expect("the Dockerfile has a runtime stage");
    let copies: Vec<&str> = runtime
        .lines()
        .filter(|line| line.starts_with("COPY "))
        .collect();
    assert!(
        copies
            .iter()
            .any(|line| line.ends_with(" /usr/local/bin/crystalline")),
        "the binary goes to /usr/local/bin, which the marker path below assumes"
    );
    let line =
        format!("COPY --chmod=0444 docker/{MARKER_FILE} /usr/local/lib/crystalline/{MARKER_FILE}");
    assert!(copies.contains(&line.as_str()), "no `{line}` in {copies:?}");
    assert!(
        !dockerfile.lines().any(|line| line.starts_with("RUN ")),
        "the image has no RUN step"
    );
    assert_eq!(
        marker(&format!("docker/{MARKER_FILE}")).as_deref(),
        Some("docker\n")
    );
    let ignored = repo_file(".dockerignore");
    assert!(
        !ignored
            .lines()
            .any(|line| line.trim_start_matches('/').starts_with("docker")),
        ".dockerignore keeps docker/ in the build context"
    );
}

/// `CRYSTALLINE_OFFICIAL_BUILD` is `1` only when build.yml is called with
/// a release tag (a dry run passes none), set in the build job and nowhere
/// else in any workflow.
#[test]
fn official_flag_only_for_tagged_builds() {
    const NAME: &str = "CRYSTALLINE_OFFICIAL_BUILD";
    let text = repo_file(".github/workflows/build.yml");
    let workflow: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).expect("build.yml parses");
    assert_eq!(
        workflow["jobs"]["build"]["env"][NAME].as_str(),
        Some("${{ inputs.tag != '' && '1' || '' }}"),
        "the build job sets the flag from the tag input"
    );
    assert_eq!(text.matches(NAME).count(), 1, "build.yml names it once");
    let workflows = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
    for entry in std::fs::read_dir(&workflows).expect("the workflows folder") {
        let path = entry.expect("a workflow entry").path();
        if path.file_name().is_some_and(|name| name == "build.yml") {
            continue;
        }
        let other = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !other.contains(NAME),
            "{} must not set {NAME}",
            path.display()
        );
    }
}
