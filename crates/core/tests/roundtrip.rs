//! Golden round-trip tests: byte-identical `parse -> emit` for canonical files,
//! lossless reconstruction for every valid file and typed errors for the
//! encoding and YAML failure fixtures.

mod common;

use common::{md_files, read, stem};
use crystalline_core::parse::ParseError;
use crystalline_core::{emit_engram, parse_engram, parse_engram_lossless};

/// Directories whose files are canonical: `parse -> emit` is byte-identical.
const BYTE_IDENTICAL_DIRS: &[&str] = &["canonical", "manifests", "schemas"];
/// Directories whose valid files reconstruct losslessly but are not canonical.
const LOSSLESS_ONLY_DIRS: &[&str] = &["lossless"];

#[test]
fn canonical_files_emit_byte_identical() {
    let mut count = 0;
    for dir in BYTE_IDENTICAL_DIRS {
        for path in md_files(dir) {
            let source = read(&path);
            let engram = parse_engram(&source)
                .unwrap_or_else(|e| panic!("{}: parse failed: {e}", path.display()));
            let emitted = emit_engram(&engram);
            assert_eq!(
                emitted,
                source,
                "parse -> emit not byte-identical for {}",
                path.display()
            );
            count += 1;
        }
    }
    assert!(count >= 25, "expected many canonical fixtures, got {count}");
}

#[test]
fn valid_files_reconstruct_losslessly() {
    for dir in BYTE_IDENTICAL_DIRS.iter().chain(LOSSLESS_ONLY_DIRS) {
        for path in md_files(dir) {
            let source = read(&path);
            let lossless = parse_engram_lossless(&source)
                .unwrap_or_else(|e| panic!("{}: lossless parse failed: {e}", path.display()));
            assert_eq!(
                lossless.reconstruct(),
                source,
                "lossless reconstruct differs for {}",
                path.display()
            );
        }
    }
}

#[test]
fn lossless_only_files_are_not_canonical() {
    // These files parse fine but a full emit intentionally normalizes them, so
    // they must differ from a byte-identical emit while still reconstructing.
    for path in md_files("lossless") {
        let source = read(&path);
        let engram = parse_engram(&source).unwrap();
        assert_ne!(
            emit_engram(&engram),
            source,
            "expected {} to be non-canonical",
            stem(&path)
        );
    }
}

#[test]
fn bom_fixture_is_rejected() {
    let source = read(
        &md_files("errors")
            .into_iter()
            .find(|p| stem(p) == "bom")
            .unwrap(),
    );
    assert_eq!(parse_engram(&source), Err(ParseError::Bom));
}

#[test]
fn null_byte_fixture_is_rejected() {
    let source = read(
        &md_files("errors")
            .into_iter()
            .find(|p| stem(p) == "null-byte")
            .unwrap(),
    );
    match parse_engram(&source) {
        Err(ParseError::NullByte { line, column }) => {
            assert!(line >= 1 && column >= 1);
        }
        other => panic!("expected NullByte error, got {other:?}"),
    }
}

#[test]
fn broken_yaml_fixture_is_rejected() {
    let source = read(
        &md_files("errors")
            .into_iter()
            .find(|p| stem(p) == "broken-yaml")
            .unwrap(),
    );
    assert!(matches!(
        parse_engram(&source),
        Err(ParseError::Yaml { .. })
    ));
}
