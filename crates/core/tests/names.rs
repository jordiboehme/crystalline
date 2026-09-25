//! The name table: every spelling of a domain (local name, canonical name,
//! alias) resolves to one local name, with local over canonical over alias.

use crystalline_core::names::{DroppedAlias, NameConflict, NameInput, NameTable};

fn input(local: &str, canonical: Option<&str>, aliases: &[&str]) -> NameInput {
    NameInput {
        local: local.into(),
        canonical: canonical.map(Into::into),
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    }
}

#[test]
fn a_local_name_always_resolves_to_itself() {
    let t = NameTable::build(&[input("eng", None, &[])]);
    assert_eq!(t.resolve("eng"), Some("eng"));
    assert_eq!(t.canonical("eng"), Some("eng"));
    assert_eq!(t.resolve("ENG"), None, "exact spelling only");
    assert_eq!(t.resolve(" eng"), None, "exact spelling only");
    assert_eq!(t.canonical("unknown"), None);
}

#[test]
fn a_canonical_name_resolves_to_its_domain() {
    let t = NameTable::build(&[input("eng-knowledge", Some("eng"), &[])]);
    assert_eq!(t.resolve("eng"), Some("eng-knowledge"));
    assert_eq!(t.resolve("eng-knowledge"), Some("eng-knowledge"));
    assert_eq!(t.canonical("eng-knowledge"), Some("eng"));
    assert!(!t.is_shadowed("eng-knowledge"));
}

#[test]
fn a_local_name_beats_another_domains_canonical_and_shadows_it() {
    let t = NameTable::build(&[
        input("platform", None, &[]),
        input("platform-2", Some("platform"), &[]),
    ]);
    assert_eq!(t.resolve("platform"), Some("platform"));
    assert!(t.is_shadowed("platform-2"));
    assert!(!t.is_shadowed("platform"));
    assert_eq!(t.canonical("platform-2"), Some("platform"));
}

#[test]
fn two_claimants_of_one_unregistered_name_resolve_nowhere() {
    let t = NameTable::build(&[
        input("b", Some("shared"), &[]),
        input("a", Some("shared"), &[]),
    ]);
    assert_eq!(t.resolve("shared"), None);
    assert_eq!(
        t.conflicts(),
        &[NameConflict {
            name: "shared".into(),
            claimants: vec!["a".into(), "b".into()]
        }]
    );
    assert!(!t.is_shadowed("a"));
    assert_eq!(t.canonical("a"), Some("shared"));
    assert_eq!(
        t.normalize("a"),
        None,
        "a contested canonical is never written"
    );
}

#[test]
fn an_alias_resolves_unless_its_spelling_is_taken() {
    let t = NameTable::build(&[
        input("eng", None, &["eng-old", "ops"]),
        input("ops", None, &[]),
        input("x", None, &["dup"]),
        input("y", None, &["dup"]),
    ]);
    assert_eq!(t.resolve("eng-old"), Some("eng"));
    assert_eq!(t.resolve("ops"), Some("ops"));
    assert_eq!(t.aliases("eng"), &["eng-old".to_string()]);
    assert_eq!(t.resolve("dup"), None);
    let dropped = t.dropped_aliases();
    assert!(dropped.contains(&DroppedAlias {
        domain: "eng".into(),
        alias: "ops".into(),
        held_by: Some("ops".into())
    }));
    assert!(dropped.contains(&DroppedAlias {
        domain: "x".into(),
        alias: "dup".into(),
        held_by: None
    }));
    assert!(dropped.contains(&DroppedAlias {
        domain: "y".into(),
        alias: "dup".into(),
        held_by: None
    }));
}

#[test]
fn an_alias_never_beats_a_canonical_name() {
    let t = NameTable::build(&[input("a", Some("core"), &[]), input("b", None, &["core"])]);
    assert_eq!(t.resolve("core"), Some("a"));
    assert_eq!(t.aliases("b"), &[] as &[String]);
    assert_eq!(
        t.dropped_aliases(),
        &[DroppedAlias {
            domain: "b".into(),
            alias: "core".into(),
            held_by: Some("a".into())
        }]
    );
}

#[test]
fn an_alias_on_a_contested_name_is_dropped_without_an_owner() {
    let t = NameTable::build(&[
        input("a", Some("shared"), &[]),
        input("b", Some("shared"), &[]),
        input("c", None, &["shared"]),
    ]);
    assert_eq!(t.resolve("shared"), None);
    assert_eq!(
        t.dropped_aliases(),
        &[DroppedAlias {
            domain: "c".into(),
            alias: "shared".into(),
            held_by: None
        }]
    );
}

#[test]
fn an_invalid_canonical_is_ignored() {
    let t = NameTable::build(&[
        input("eng", Some("not a name"), &[]),
        input("ops", Some(""), &[]),
    ]);
    assert_eq!(t.canonical("eng"), Some("eng"));
    assert_eq!(t.canonical("ops"), Some("ops"));
    assert_eq!(t.resolve("not a name"), None);
    assert!(t.conflicts().is_empty());
    assert!(!t.is_shadowed("eng"));
}

#[test]
fn an_alias_repeating_the_domains_own_names_is_skipped_silently() {
    let t = NameTable::build(&[input(
        "eng-knowledge",
        Some("eng"),
        &["eng", "eng-knowledge", "old", "old"],
    )]);
    assert_eq!(t.aliases("eng-knowledge"), &["old".to_string()]);
    assert!(t.dropped_aliases().is_empty());
    assert_eq!(t.resolve("old"), Some("eng-knowledge"));
}

#[test]
fn a_shadowed_domain_keeps_its_own_aliases() {
    let t = NameTable::build(&[
        input("platform", None, &[]),
        input("platform-2", Some("platform"), &["plat", "platform"]),
    ]);
    assert_eq!(t.aliases("platform-2"), &["plat".to_string()]);
    assert_eq!(t.resolve("plat"), Some("platform-2"));
    assert!(
        t.dropped_aliases().is_empty(),
        "its own canonical is skipped"
    );
}

#[test]
fn reports_are_sorted_whatever_the_input_order() {
    let t = NameTable::build(&[
        input("z", Some("two"), &["dup"]),
        input("y", Some("one"), &["dup"]),
        input("x", Some("two"), &[]),
        input("w", Some("one"), &[]),
    ]);
    let names: Vec<&str> = t.conflicts().iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["one", "two"]);
    assert_eq!(t.conflicts()[0].claimants, ["w", "y"]);
    let dropped: Vec<&str> = t
        .dropped_aliases()
        .iter()
        .map(|d| d.domain.as_str())
        .collect();
    assert_eq!(dropped, ["y", "z"]);
}

#[test]
fn normalize_points_only_at_a_canonical_that_comes_back() {
    let t = NameTable::build(&[
        input("eng-knowledge", Some("eng"), &["engineering"]),
        input("platform", None, &[]),
        input("platform-2", Some("platform"), &["plat"]),
    ]);
    assert_eq!(t.normalize("eng-knowledge"), Some("eng"));
    assert_eq!(t.normalize("engineering"), Some("eng"));
    assert_eq!(t.normalize("eng"), None, "already canonical");
    assert_eq!(
        t.normalize("plat"),
        None,
        "canonical 'platform' resolves to another domain here"
    );
    assert_eq!(t.normalize("platform"), None);
    assert_eq!(t.normalize("unknown"), None);
}

#[test]
fn spellings_list_every_resolvable_spelling_once() {
    let t = NameTable::build(&[input("eng-knowledge", Some("eng"), &["old"])]);
    assert_eq!(
        t.spellings(),
        vec![
            ("eng".to_string(), "eng-knowledge".to_string()),
            ("eng-knowledge".to_string(), "eng-knowledge".to_string()),
            ("old".to_string(), "eng-knowledge".to_string()),
        ]
    );
}

#[test]
fn an_empty_table_resolves_nothing() {
    let t = NameTable::default();
    assert_eq!(t.resolve("eng"), None);
    assert!(t.spellings().is_empty());
    assert_eq!(t.aliases("eng"), &[] as &[String]);
}
