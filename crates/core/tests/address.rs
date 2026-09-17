//! Slugification, `crystalline://` URL parsing and link resolution.

use crystalline_core::address::{CrystallineUrl, LookupTable, Resolution, resolve, slugify};
use crystalline_core::engram::LinkTarget;

#[test]
fn slugify_basic_path() {
    assert_eq!(
        slugify("Astronomy/Phobos Orbit.md"),
        "astronomy/phobos-orbit"
    );
}

#[test]
fn slugify_collapses_and_trims() {
    assert_eq!(slugify("  Weird__Name!! .md"), "weird-name");
    assert_eq!(slugify("A/  B  /C.md"), "a/b/c");
    assert_eq!(slugify("--leading--/trailing--.md"), "leading/trailing");
    assert_eq!(slugify("Already/Sluggy-Path"), "already/sluggy-path");
}

#[test]
fn slugify_drops_empty_segments() {
    assert_eq!(slugify("a///b.md"), "a/b");
}

#[test]
fn url_round_trips() {
    let u = CrystallineUrl::parse("crystalline://astronomy/phobos-orbit").unwrap();
    assert_eq!(u.domain, "astronomy");
    assert_eq!(u.permalink, "phobos-orbit");
    assert!(!u.glob);
    assert_eq!(u.to_url(), "crystalline://astronomy/phobos-orbit");
}

#[test]
fn url_nested_permalink() {
    let u = CrystallineUrl::parse("crystalline://product/schemas/task-schema").unwrap();
    assert_eq!(u.domain, "product");
    assert_eq!(u.permalink, "schemas/task-schema");
    assert_eq!(u.to_url(), "crystalline://product/schemas/task-schema");
}

#[test]
fn url_glob_forms() {
    let all = CrystallineUrl::parse("crystalline://astronomy/*").unwrap();
    assert!(all.glob);
    assert_eq!(all.permalink, "");
    assert_eq!(all.to_url(), "crystalline://astronomy/*");
    assert!(all.matches("astronomy", "anything/here"));
    assert!(!all.matches("gardening", "anything"));

    let sub = CrystallineUrl::parse("crystalline://astronomy/messier/*").unwrap();
    assert!(sub.glob);
    assert_eq!(sub.permalink, "messier/");
    assert_eq!(sub.to_url(), "crystalline://astronomy/messier/*");
    assert!(sub.matches("astronomy", "messier/m31"));
    assert!(!sub.matches("astronomy", "other/m31"));
}

#[test]
fn url_rejects_malformed() {
    assert!(CrystallineUrl::parse("http://astronomy/x").is_none());
    assert!(CrystallineUrl::parse("crystalline:///no-domain").is_none());
}

#[test]
fn url_names_an_attachment_only_under_assets() {
    let asset = CrystallineUrl::parse("crystalline://d/assets/a/b.png").unwrap();
    assert_eq!(asset.asset_path(), Some("assets/a/b.png"));

    let engram = CrystallineUrl::parse("crystalline://d/notes/a-b").unwrap();
    assert_eq!(engram.asset_path(), None);

    let glob = CrystallineUrl::parse("crystalline://d/assets/*").unwrap();
    assert_eq!(glob.asset_path(), None);
}

fn table() -> LookupTable {
    let mut t = LookupTable::new();
    t.insert("astronomy", "astronomy/phobos-orbit", "Phobos Orbit");
    t.insert("astronomy", "astronomy/kepler-laws", "Kepler Laws");
    t.insert(
        "gardening",
        "gardening/composting-basics",
        "Composting Basics",
    );
    t.insert(
        "astronomy",
        "astronomy/log-weekly",
        "Log: Weekly Garden Notes",
    );
    t
}

#[test]
fn resolve_permalink_in_current_domain() {
    let target = LinkTarget::parse("astronomy/phobos-orbit");
    match resolve(&target, "astronomy", &table()) {
        Resolution::Resolved(r) => {
            assert_eq!(r.domain, "astronomy");
            assert_eq!(r.permalink, "astronomy/phobos-orbit");
        }
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn resolve_title_in_current_domain() {
    let target = LinkTarget::parse("Kepler Laws");
    match resolve(&target, "astronomy", &table()) {
        Resolution::Resolved(r) => assert_eq!(r.permalink, "astronomy/kepler-laws"),
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn bare_title_never_resolves_cross_domain() {
    // "Composting Basics" lives in gardening; a bare link from astronomy must
    // not reach it.
    let target = LinkTarget::parse("Composting Basics");
    assert_eq!(
        resolve(&target, "astronomy", &table()),
        Resolution::Unresolved
    );
}

#[test]
fn explicit_domain_prefix_resolves() {
    let target = LinkTarget::parse("gardening:Composting Basics");
    match resolve(&target, "astronomy", &table()) {
        Resolution::Resolved(r) => {
            assert_eq!(r.domain, "gardening");
            assert_eq!(r.permalink, "gardening/composting-basics");
        }
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn explicit_domain_miss_is_cross_domain_unresolved() {
    let target = LinkTarget::parse("gardening:Nonexistent");
    assert_eq!(
        resolve(&target, "astronomy", &table()),
        Resolution::CrossDomainUnresolved {
            domain: "gardening".into()
        }
    );
}

#[test]
fn title_whose_first_word_ends_in_a_colon_resolves_at_home() {
    // The parse cannot help splitting this into a domain and a target: `Log`
    // has no whitespace and something follows the colon. Nothing registered a
    // domain called `Log`, though, so the whole bracket text is a title, and
    // that is what the resolver falls back to.
    let target = LinkTarget::parse("Log: Weekly Garden Notes");
    assert_eq!(target.domain.as_deref(), Some("Log"));
    match resolve(&target, "astronomy", &table()) {
        Resolution::Resolved(r) => {
            assert_eq!(r.domain, "astronomy");
            assert_eq!(r.permalink, "astronomy/log-weekly");
        }
        other => panic!("expected resolved, got {other:?}"),
    }
}

#[test]
fn unknown_prefix_matching_nothing_stays_cross_domain_unresolved() {
    // The fallback is a second question, not a softer answer: a prefix that
    // names no domain and a whole string that names no engram is still a link
    // pointing at a domain nobody has.
    let target = LinkTarget::parse("Log: Something Nobody Wrote");
    assert_eq!(
        resolve(&target, "astronomy", &table()),
        Resolution::CrossDomainUnresolved {
            domain: "Log".into()
        }
    );
}

#[test]
fn a_real_domain_prefix_is_never_read_as_a_title() {
    // `gardening` is a domain, so the prefix keeps its meaning even though
    // "gardening: Composting Basics" would be a perfectly good title. The
    // registry decides, and the fallback never runs for a name in it.
    let mut t = table();
    t.insert(
        "astronomy",
        "astronomy/decoy",
        "gardening: Composting Basics",
    );
    match resolve(
        &LinkTarget::parse("gardening: Composting Basics"),
        "astronomy",
        &t,
    ) {
        Resolution::Resolved(r) => {
            assert_eq!(r.domain, "gardening");
            assert_eq!(r.permalink, "gardening/composting-basics");
        }
        other => panic!("expected resolved, got {other:?}"),
    }
}
