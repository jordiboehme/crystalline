//! The shared asset-reference corpus: one fixture, two scanners.
//!
//! fluid/src/editor/imageFormat.test.ts runs the same file through
//! `assetRefsIn`, so a behavior change in either scanner that the other
//! does not mirror fails one of the two suites instead of shipping as a
//! silent divergence (the class defect F2 belonged to).

use crystalline_core::attachment::find_asset_refs;

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    body: String,
    refs: Vec<String>,
}

#[test]
fn the_scanner_agrees_with_the_shared_corpus() {
    let raw = include_str!("fixtures/asset_ref_corpus.json");
    let cases: Vec<Case> = serde_json::from_str(raw).expect("corpus parses");
    assert!(cases.len() >= 16, "corpus lost cases");
    for case in cases {
        assert_eq!(
            find_asset_refs(&case.body),
            case.refs,
            "case '{}' diverged",
            case.name
        );
    }
}
