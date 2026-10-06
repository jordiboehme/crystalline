//! Every first segment the server or Fluid answers at is on the reserved
//! list, so a prefix that would be stripped twice behind a stripping proxy is
//! refused when it is set. Reads `fluid/` at run time; a checkout without it
//! skips the test rather than failing the build.

use std::path::PathBuf;

use crystalline_core::base::RESERVED_FIRST_SEGMENTS;

fn fluid() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fluid")
}

fn reserved(segment: &str) -> bool {
    RESERVED_FIRST_SEGMENTS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(segment))
}

#[test]
fn every_top_level_route_of_fluid_is_reserved() {
    let Ok(routes) = std::fs::read_to_string(fluid().join("src/routes.tsx")) else {
        eprintln!("note: skipping, this checkout does not carry fluid/");
        return;
    };
    let mut seen = 0;
    for (at, _) in routes.match_indices("path=\"/") {
        let rest = &routes[at + "path=\"/".len()..];
        let end = rest.find(['/', '"', '*', ':']).unwrap_or(rest.len());
        let segment = &rest[..end];
        if segment.is_empty() {
            continue;
        }
        seen += 1;
        assert!(
            reserved(segment),
            "the Fluid route /{segment} is not in RESERVED_FIRST_SEGMENTS: add it there, \
             or put the route under an existing first segment"
        );
    }
    assert!(seen >= 10, "the route table was not read: {seen} routes");
}

#[test]
fn every_root_level_file_of_the_bundle_is_reserved() {
    let Ok(entries) = std::fs::read_dir(fluid().join("public")) else {
        eprintln!("note: skipping, this checkout does not carry fluid/public");
        return;
    };
    for entry in entries {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(
            reserved(&name),
            "fluid/public/{name} is not in RESERVED_FIRST_SEGMENTS"
        );
    }
}

#[test]
fn the_server_first_segments_are_reserved() {
    for segment in ["api", "assets", "health", ".well-known"] {
        assert!(reserved(segment), "{segment}");
    }
}
