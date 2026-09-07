//! What a scoped read may see.
//!
//! The engine's read verbs take the caller's [`Scope`] and answer from the
//! domains that caller may read. What this file pins is one property in several
//! places: a domain somebody may not see is answered exactly as a domain nobody
//! registered. Not "forbidden", not an empty result with a different shape -
//! the same bytes, because the existence of a private domain is the secret it
//! keeps.
//!
//! The policy that decides who sees what lives in `crate::scope` and its own
//! tests; the wiring between it and the engine is pinned by
//! `tests/domain_access.rs`. This is the third leg: the verbs.

mod support;

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::params::{
    BrowseParams, ContextParams, EvolveParams, InferParams, ListDomainsParams, ReadParams,
    RecentParams, SearchParams, ValidateParams, VocabularyParams,
};
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use crystalline_service::{DomainAccess, Engine, Scope};
use tokio::sync::Mutex;

const OPEN_MANIFEST: &str = "---\ntype: manifest\ntitle: open\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# open\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for shared questions\n";
const LAB_MANIFEST: &str = "---\ntype: manifest\ntitle: lab\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# lab\n\n## Scope\n\n- The private domain\n\n## When to Use\n\n- Route here for confidential lab questions\n";
const OPEN_NOTE: &str = "---\ntype: engram\ntitle: Open Note\npermalink: open-note\ntags:\n  - shared\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Open Note\n\n- [decision] the shared thing is public #shared\n";
/// The private engram: its title, its tag, its observation category and the
/// word `secret` are all things a stranger must not be able to read back out of
/// any verb. Its relation points into `open`, so it is also an inbound
/// reference the shared engram must not report.
const LAB_NOTE: &str = "---\ntype: dossier\ntitle: Lab Note\npermalink: lab-note\ntags:\n  - confidential\nstatus: stable\nrecorded_at: 2026-01-03\n---\n\n# Lab Note\n\n- [secret] the secret formula is here #confidential\n- relates_to [[open:Open Note]]\n";
/// A second private engram, written so the maintenance sweep has something to
/// find in `lab`: its relation resolves to nothing, which is an unresolved
/// reference every detector run reports by domain, permalink and path.
const LAB_DRAFT: &str = "---\ntype: dossier\ntitle: Lab Draft\npermalink: lab-draft\ntags:\n  - confidential\nstatus: stable\nrecorded_at: 2026-01-04\n---\n\n# Lab Draft\n\n- [secret] the draft points nowhere yet #confidential\n- relates_to [[Nothing Here At All]]\n";

/// Two file domains, `open` and `lab`, each with a MANIFEST and one engram,
/// plus an accounts store where `lab` is private to `owner` with `mem` invited.
/// `out` is a signed-in stranger.
async fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    for (name, manifest, note) in [
        ("open", OPEN_MANIFEST, OPEN_NOTE),
        ("lab", LAB_MANIFEST, LAB_NOTE),
    ] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), manifest).unwrap();
        std::fs::write(dir.join(format!("{name}-note.md")), note).unwrap();
        if name == "lab" {
            std::fs::write(dir.join("lab-draft.md"), LAB_DRAFT).unwrap();
        }
        cfg.domains.insert(name.to_string(), DomainEntry::file(dir));
    }
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();

    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role) in [
        ("owner", Role::Editor),
        ("mem", Role::Editor),
        ("out", Role::Editor),
        ("boss", Role::Admin),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "owner")
        .await
        .unwrap();
    auth.upsert_domain_member("lab", "mem", MemberLevel::Viewer, "owner")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));
    (tmp, engine)
}

fn user(account: &str) -> Scope {
    Scope::User {
        account: account.into(),
        admin: false,
    }
}

/// An instance admin, with the flag the surface resolved carried along. An
/// admin already manages every account here, so a domain it could not see would
/// be a secret kept from the person who can grant themselves the account that
/// holds it - the resolver says so, and these verbs have to agree.
fn admin(account: &str) -> Scope {
    Scope::User {
        account: account.into(),
        admin: true,
    }
}

fn read(identifier: &str, domain: &str) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: Some(domain.to_string()),
    }
}

fn browse(domain: &str) -> BrowseParams {
    BrowseParams {
        domain: domain.to_string(),
        path: None,
        depth: None,
        glob: None,
    }
}

/// The task's headline property, over the verbs an agent arrives through: a
/// private domain is absent from the index, absent from search and unreadable,
/// and unreadable in the same words an engram nobody wrote is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hidden_domain_is_absent_from_search_list_and_read() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");

    let listed = engine
        .list_domains(&ListDomainsParams::default(), &stranger)
        .await
        .unwrap();
    assert!(
        !listed.to_string().contains("lab") && listed.to_string().contains("open"),
        "the index names what the caller may route to and nothing else: {listed}"
    );

    let hits = engine
        .search_engrams(
            &SearchParams {
                query: Some("secret".to_string()),
                ..SearchParams::default()
            },
            &stranger,
        )
        .await
        .unwrap();
    assert!(
        !hits.to_string().contains("lab"),
        "an unfiltered search never reaches into a hidden domain: {hits}"
    );

    // The equality that matters: the read of a hidden engram and the read of the
    // same identifier in a domain nobody registered differ by the domain name
    // and by nothing else.
    let hidden = engine
        .read_engram(&read("lab-note", "lab"), &stranger)
        .await
        .unwrap_err();
    let absent = engine
        .read_engram(&read("lab-note", "nope"), &stranger)
        .await
        .unwrap_err();
    assert_eq!(
        hidden.to_string(),
        absent.to_string().replace("nope", "lab"),
        "hidden reads as absent, not as forbidden"
    );

    // And the machine owner is untouched by all of it.
    let owner_view = engine
        .list_domains(&ListDomainsParams::default(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        owner_view.to_string().contains("lab"),
        "the machine owner sees everything: {owner_view}"
    );
    assert!(
        engine
            .read_engram(&read("lab-note", "lab"), &Scope::Unrestricted)
            .await
            .is_ok()
    );
}

/// An invitation is what makes the difference, not the shape of the request:
/// the member and the owner read exactly what the stranger cannot - and so does
/// an instance admin, who is never a member of anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_and_the_owner_see_the_private_domain() {
    let (_tmp, engine) = fixture().await;
    for (account, scope) in [
        ("mem", user("mem")),
        ("owner", user("owner")),
        ("boss", admin("boss")),
    ] {
        let listed = engine
            .list_domains(&ListDomainsParams::default(), &scope)
            .await
            .unwrap();
        assert!(listed.to_string().contains("lab"), "{account}: {listed}");
        let hits = engine
            .search_engrams(
                &SearchParams {
                    query: Some("secret".to_string()),
                    ..SearchParams::default()
                },
                &scope,
            )
            .await
            .unwrap();
        assert!(hits.to_string().contains("lab-note"), "{account}: {hits}");
        assert!(
            engine
                .read_engram(&read("lab-note", "lab"), &scope)
                .await
                .is_ok(),
            "{account} reads the private engram"
        );
    }
}

/// Naming the domain explicitly is not a way around it - in either direction.
/// A filter that names only a hidden domain answers empty, exactly as a filter
/// naming a domain nobody registered does; a filter that names both keeps the
/// visible half.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_filter_naming_a_hidden_domain_answers_like_an_unknown_one() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");

    let only_hidden = engine
        .search_engrams(
            &SearchParams {
                domains: vec!["lab".to_string()],
                ..SearchParams::default()
            },
            &stranger,
        )
        .await
        .unwrap();
    let only_unknown = engine
        .search_engrams(
            &SearchParams {
                domains: vec!["nope".to_string()],
                ..SearchParams::default()
            },
            &stranger,
        )
        .await
        .unwrap();
    assert_eq!(
        only_hidden, only_unknown,
        "a hidden domain and an unregistered one search the same: {only_hidden}"
    );
    assert_eq!(only_hidden["total"], 0);

    // Not by refusing everything: the visible half of a mixed filter answers.
    let mixed = engine
        .search_engrams(
            &SearchParams {
                domains: vec!["lab".to_string(), "open".to_string()],
                ..SearchParams::default()
            },
            &stranger,
        )
        .await
        .unwrap();
    assert!(
        mixed.to_string().contains("open-note") && !mixed.to_string().contains("lab-note"),
        "the visible half of the filter still answers: {mixed}"
    );

    // The same rule on the activity feed, whose row limit is enforced in SQL.
    let recent = engine
        .recent_activity(
            &RecentParams {
                domains: Vec::new(),
                timeframe: Some("100y".to_string()),
                types: Vec::new(),
            },
            &stranger,
        )
        .await
        .unwrap();
    assert!(
        recent.to_string().contains("open-note") && !recent.to_string().contains("lab-note"),
        "recent activity covers what the caller may read: {recent}"
    );
    let hidden_only = engine
        .recent_activity(
            &RecentParams {
                domains: vec!["lab".to_string()],
                timeframe: Some("100y".to_string()),
                types: Vec::new(),
            },
            &stranger,
        )
        .await
        .unwrap();
    let unknown_only = engine
        .recent_activity(
            &RecentParams {
                domains: vec!["nope".to_string()],
                timeframe: Some("100y".to_string()),
                types: Vec::new(),
            },
            &stranger,
        )
        .await
        .unwrap();
    assert_eq!(
        hidden_only, unknown_only,
        "the whole envelope matches, not just the count: {hidden_only}"
    );
    assert_eq!(hidden_only["count"], 0, "{hidden_only}");
}

/// Browsing a hidden domain is refused with the unregistered-domain error, and
/// that error does not name the private domains in the registered set it lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browsing_a_hidden_domain_is_the_unknown_domain_error() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");
    let hidden = engine
        .browse_domain(&browse("lab"), &stranger)
        .await
        .unwrap_err()
        .to_string();
    let absent = engine
        .browse_domain(&browse("nope"), &stranger)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        hidden,
        absent.replace("'nope'", "'lab'"),
        "one error for both, so the miss says nothing about which it was"
    );
    assert!(
        !absent.contains("lab"),
        "and the registered set it names leaves the private domain out: {absent}"
    );
    assert!(
        engine
            .browse_domain(&browse("lab"), &user("mem"))
            .await
            .is_ok(),
        "the member browses it"
    );

    // The bare domain check a surface runs before a domain-addressed read
    // raises the same error, and it is the one place a single unfiltered answer
    // would name every private domain at once.
    let checked = engine
        .require_domain("nope", &stranger)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !checked.contains("lab"),
        "the domain check names the visible set and nothing else: {checked}"
    );
    assert_eq!(
        engine
            .require_domain("lab", &stranger)
            .await
            .unwrap_err()
            .to_string(),
        checked.replace("'nope'", "'lab'"),
        "and a hidden domain fails it exactly as an unregistered one"
    );
    assert!(engine.require_domain("lab", &user("mem")).await.is_ok());
}

/// Tag names and their counts are content. An all-domain vocabulary sweep for a
/// caller who may not read every domain covers the ones it may, and a sweep
/// named at a hidden domain reports the empty vocabulary an unknown one does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vocabulary_sweep_covers_only_visible_domains() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");

    let all = engine
        .vocabulary(&VocabularyParams { domain: None }, &stranger)
        .await
        .unwrap();
    assert!(
        all.to_string().contains("shared") && !all.to_string().contains("confidential"),
        "the sweep sums the visible domains only: {all}"
    );

    let named = engine
        .vocabulary(
            &VocabularyParams {
                domain: Some("lab".to_string()),
            },
            &stranger,
        )
        .await
        .unwrap();
    let unknown = engine
        .vocabulary(
            &VocabularyParams {
                domain: Some("nope".to_string()),
            },
            &stranger,
        )
        .await
        .unwrap();
    assert_eq!(
        named,
        unknown
            .to_string()
            .replace("nope", "lab")
            .parse::<serde_json::Value>()
            .unwrap(),
        "a hidden domain reports the empty vocabulary an unknown one reports: {named}"
    );

    // The owner's sweep is the whole index, and it is the same single query it
    // always was.
    let owner_view = engine
        .vocabulary(&VocabularyParams { domain: None }, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        owner_view.to_string().contains("confidential"),
        "{owner_view}"
    );
    assert_eq!(
        owner_view,
        engine
            .vocabulary(&VocabularyParams { domain: None }, &user("mem"))
            .await
            .unwrap(),
        "and a member's merged sweep is that same answer, name for name"
    );
}

/// The graph is where a private engram leaks without being read: it is a
/// neighbour of something public. An anchor in a hidden domain is not found,
/// and a hidden neighbour of a visible anchor is not in the slice - nor is the
/// edge that would have pointed at it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_neighbourhood_stops_at_a_hidden_domain() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");

    let hidden_anchor = engine
        .build_context(
            &ContextParams {
                anchor: "crystalline://lab/lab-note".to_string(),
                depth: Some(2),
                domains: Vec::new(),
                timeframe: None,
                max_related: None,
            },
            &stranger,
        )
        .await
        .unwrap_err();
    let absent_anchor = engine
        .build_context(
            &ContextParams {
                anchor: "crystalline://nope/lab-note".to_string(),
                depth: Some(2),
                domains: Vec::new(),
                timeframe: None,
                max_related: None,
            },
            &stranger,
        )
        .await
        .unwrap_err();
    assert_eq!(
        hidden_anchor.to_string(),
        absent_anchor.to_string().replace("nope", "lab"),
        "an anchor in a hidden domain is an anchor that is not there"
    );

    let from_open = engine
        .build_context(
            &ContextParams {
                anchor: "crystalline://open/open-note".to_string(),
                depth: Some(2),
                domains: Vec::new(),
                timeframe: None,
                max_related: None,
            },
            &stranger,
        )
        .await
        .unwrap();
    assert!(
        !from_open.to_string().contains("lab"),
        "the private neighbour is cut out of the slice: {from_open}"
    );
    assert_eq!(
        from_open["edges"].as_array().map(|e| e.len()),
        Some(0),
        "and so is the edge that would have pointed at it: {from_open}"
    );

    let graph = engine
        .graph_neighborhood("crystalline://open/open-note", 2, 100, &stranger)
        .await
        .unwrap();
    assert!(
        !graph.to_string().contains("lab"),
        "the graph view answers the same: {graph}"
    );
    assert_eq!(
        graph["hidden"], 0,
        "and does not count what it never drew as capped away: {graph}"
    );
    assert!(
        engine
            .graph_neighborhood("crystalline://open/open-note", 2, 100, &user("mem"))
            .await
            .unwrap()
            .to_string()
            .contains("lab-note"),
        "the member sees the whole neighbourhood"
    );
}

/// The read payload's own cross-domain surface: who points here. A reference
/// out of a hidden domain names that domain and one of its file paths, so it is
/// gone from the sample and from the count the sample is drawn from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inbound_reference_from_a_hidden_domain_is_not_reported() {
    let (_tmp, engine) = fixture().await;
    let owner_view = engine
        .read_engram(&read("open-note", "open"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        owner_view["inbound"]["count"], 1,
        "the reference is really there: {owner_view}"
    );

    let stranger_view = engine
        .read_engram(&read("open-note", "open"), &user("out"))
        .await
        .unwrap();
    assert!(
        stranger_view.get("inbound").is_none(),
        "with the only inbound reference dropped, the block is omitted entirely: {stranger_view}"
    );
    assert!(
        !stranger_view.to_string().contains("lab"),
        "{stranger_view}"
    );
}

/// The routing block an agent is onboarded with: a domain it may not read must
/// not be routed to, and its MANIFEST bullets are as much of a disclosure as
/// its name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_routing_block_drops_a_hidden_domain() {
    let (_tmp, engine) = fixture().await;
    let scoped = engine.routing_text_scoped(&user("out")).await.unwrap();
    assert!(
        scoped.contains("open") && !scoped.contains("lab"),
        "no line, and no bullet, for a domain the caller may not read: {scoped}"
    );
    assert!(
        !scoped.contains("confidential"),
        "the MANIFEST prose goes with it: {scoped}"
    );
    let member = engine.routing_text_scoped(&user("mem")).await.unwrap();
    assert!(
        member.contains("lab"),
        "the member is routed to it: {member}"
    );
    assert_eq!(
        engine
            .routing_text_scoped(&Scope::Unrestricted)
            .await
            .unwrap(),
        engine.routing_text(),
        "and the machine owner's scoped block is the unscoped one"
    );
}

/// The maintenance sweep is a read that enumerates every registered domain and
/// names the domain, permalink and path of everything it finds. Scoped like the
/// rest: the sweep covers what the caller may read, and naming a hidden domain
/// fails exactly as naming an unregistered one does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_evolve_sweep_covers_only_visible_domains() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");

    let swept = engine
        .evolve_engrams(&EvolveParams::default(), &stranger)
        .await
        .unwrap();
    assert_eq!(
        swept["scope"]["domains"],
        serde_json::json!(["open"]),
        "the default scope is every domain the caller may read: {swept}"
    );
    assert!(
        !swept.to_string().contains("lab"),
        "and no finding names the private domain: {swept}"
    );

    // The finding really is there for somebody who may see it, so the assertion
    // above is not passing because the sweep found nothing at all.
    let admin_view = engine
        .evolve_engrams(&EvolveParams::default(), &admin("boss"))
        .await
        .unwrap();
    assert!(
        admin_view.to_string().contains("lab-draft"),
        "an admin sweeps the private domain too: {admin_view}"
    );
    assert!(
        engine
            .evolve_engrams(&EvolveParams::default(), &user("mem"))
            .await
            .unwrap()
            .to_string()
            .contains("lab-draft"),
        "and so does the member"
    );

    let named_hidden = engine
        .evolve_engrams(
            &EvolveParams {
                domains: vec!["lab".to_string()],
                ..EvolveParams::default()
            },
            &stranger,
        )
        .await
        .unwrap_err()
        .to_string();
    let named_unknown = engine
        .evolve_engrams(
            &EvolveParams {
                domains: vec!["nope".to_string()],
                ..EvolveParams::default()
            },
            &stranger,
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        named_hidden,
        named_unknown.replace("'nope'", "'lab'"),
        "a named hidden domain fails as an unregistered one"
    );
    assert!(!named_unknown.contains("lab"), "{named_unknown}");
}

/// The two schema verbs read a named domain's engrams whole - permalinks, paths
/// and per-engram messages out of validate, field names generalized out of the
/// content by infer. Both refuse a hidden domain as an unregistered one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_schema_verbs_refuse_a_hidden_domain_as_unregistered() {
    let (_tmp, engine) = fixture().await;
    let stranger = user("out");

    let validate = |domain: &str, scope: Scope| {
        let engine = engine.clone();
        let domain = domain.to_string();
        async move {
            engine
                .validate_engrams(
                    &ValidateParams {
                        domain,
                        identifier: None,
                        engram_type: None,
                        drift: false,
                    },
                    &scope,
                )
                .await
        }
    };
    let infer = |domain: &str, scope: Scope| {
        let engine = engine.clone();
        let domain = domain.to_string();
        async move {
            engine
                .infer_schema(
                    &InferParams {
                        domain,
                        engram_type: "dossier".to_string(),
                        threshold: None,
                    },
                    &scope,
                )
                .await
        }
    };

    let hidden = validate("lab", stranger.clone())
        .await
        .unwrap_err()
        .to_string();
    let unknown = validate("nope", stranger.clone())
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(hidden, unknown.replace("'nope'", "'lab'"), "validate");
    assert!(!unknown.contains("lab"), "{unknown}");

    let hidden = infer("lab", stranger.clone())
        .await
        .unwrap_err()
        .to_string();
    let unknown = infer("nope", stranger.clone())
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(hidden, unknown.replace("'nope'", "'lab'"), "infer");

    // Both answer for somebody who may read the domain.
    assert!(validate("lab", admin("boss")).await.is_ok());
    assert!(validate("lab", user("mem")).await.is_ok());
    let inferred = infer("lab", user("mem")).await.unwrap();
    assert!(
        inferred.to_string().contains("dossier"),
        "the member infers over the private domain: {inferred}"
    );
}
