//! `crystalline desktop-bundle`: the hidden command that stages a Claude
//! Desktop extension for the release and CI. It runs on every OS.

use std::io::Read;
use std::path::Path;

use assert_cmd::Command;

fn bin(home: &Path) -> Command {
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home);
    cmd
}

fn manifest(dir: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn the_hollow_stage_holds_manifest_icon_and_shim() {
    let home = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let stage = out.path().join("stage");
    bin(home.path())
        .args(["desktop-bundle", "hollow"])
        .arg(&stage)
        .args(["--binary", "/opt/homebrew/bin/crystalline"])
        .assert()
        .success();
    assert!(stage.join("manifest.json").is_file());
    assert!(stage.join("icon.png").is_file());
    let shim = std::fs::read(stage.join("server/shim.js")).unwrap();
    let source = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/mcpb/shim/shim.js"),
    )
    .unwrap();
    assert_eq!(shim, source, "the staged launcher is the shim in the repo");
    let m = manifest(&stage);
    assert_eq!(m["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        m["server"]["mcp_config"]["env"]["CRYSTALLINE_BINARY"],
        "/opt/homebrew/bin/crystalline"
    );
}

#[test]
fn the_desktop_only_stage_leaves_the_binary_to_the_caller() {
    let home = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let stage = out.path().join("stage");
    let assert = bin(home.path())
        .args(["desktop-bundle", "desktop-only"])
        .arg(&stage)
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.starts_with("wrote "), "{stdout}");
    assert!(stdout.trim_end().ends_with("manifest.json"), "{stdout}");
    assert!(stage.join("manifest.json").is_file());
    assert!(stage.join("icon.png").is_file());
    assert!(
        !stage.join("server").exists(),
        "no server folder is written"
    );
    assert_eq!(manifest(&stage)["server"]["type"], "binary");
}

#[test]
fn the_packed_hollow_bundle_unzips_to_the_stage() {
    let home = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let stage = out.path().join("stage");
    let packed = out.path().join("hollow.mcpb");
    bin(home.path())
        .args(["desktop-bundle", "hollow"])
        .arg(&stage)
        .args(["--binary", "/opt/homebrew/bin/crystalline", "--pack"])
        .arg(&packed)
        .assert()
        .success();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&packed).unwrap()).unwrap();
    let mut names: Vec<String> = archive
        .file_names()
        .map(|n| n.unwrap().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["icon.png", "manifest.json", "server/shim.js"]);
    for name in &names {
        let mut bytes = Vec::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(
            bytes,
            std::fs::read(stage.join(name)).unwrap(),
            "{name} in the zip equals the staged file"
        );
    }
}

#[test]
fn binary_and_pack_are_refused_for_the_desktop_only_bundle() {
    let home = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let stage = out.path().join("stage");
    let binary = bin(home.path())
        .args(["desktop-bundle", "desktop-only"])
        .arg(&stage)
        .args(["--binary", "/x/crystalline"])
        .assert()
        .failure();
    let err = String::from_utf8(binary.get_output().stderr.clone()).unwrap();
    assert!(
        err.contains("--binary applies only to the hollow bundle."),
        "{err}"
    );
    let packed = bin(home.path())
        .args(["desktop-bundle", "desktop-only"])
        .arg(&stage)
        .arg("--pack")
        .arg(out.path().join("x.mcpb"))
        .assert()
        .failure();
    let err = String::from_utf8(packed.get_output().stderr.clone()).unwrap();
    assert!(
        err.contains(
            "--pack applies only to the hollow bundle: the release packs the Desktop-only bundle with its binary."
        ),
        "{err}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn install_claude_desktop_is_refused_on_linux() {
    let home = tempfile::tempdir().unwrap();
    let assert = bin(home.path())
        .args(["install", "claude-desktop", "--no-open", "--skip-skills"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr
            .contains("Claude Desktop is not available on Linux, so there is nothing to install."),
        "{stderr}"
    );
    let state = crate::common::isolated_state_dir(home.path());
    assert!(!state.join("claude-desktop.json").exists());
    assert!(!state.join("claude-desktop").exists());
}
