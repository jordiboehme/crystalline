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
//! `tests/domains/domain_access.rs`. This is the third leg: the verbs.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::TursoStore;
use crystalline_service::params::{
    BrowseParams, ContextParams, EvolveParams, InferParams, ListDomainsParams, MoveParams,
    ReadParams, RecentParams, SearchParams, ValidateParams, VocabularyParams,
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
    fixture_with(&[]).await
}

/// [`fixture`], with the named domains reviewing changes (review mode on).
async fn fixture_with(reviewing: &[&str]) -> (tempfile::TempDir, Arc<Engine>) {
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
    for name in reviewing {
        cfg.domains.get_mut(*name).unwrap().review = Some(ReviewMode::Overlay);
    }
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    // A state directory, so a reviewing domain reaches its overlay journal.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role) in [
        ("keeper", Role::Editor),
        ("mem", Role::Editor),
        ("out", Role::Editor),
        ("ed", Role::Editor),
        ("boss", Role::Admin),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    auth.upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    auth.upsert_domain_member("lab", "ed", MemberLevel::Editor, None, "keeper")
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
        share_link: None,
        include_images: false,
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

/// Every row of the listing says whether that domain is private, so a client
/// draws the badge off the index it already has rather than a second read per
/// domain.
///
/// Read by everyone who can see `lab` at all - a member, the owner, an admin
/// and the machine owner - because the fact is not a secret from anybody the
/// domain is already listed to. A stranger is not in that cast: `lab` is absent
/// from their listing entirely, which is the stronger answer this file's first
/// test pins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_listed_domain_says_whether_it_is_private() {
    let (_tmp, engine) = fixture().await;
    for (who, scope) in [
        ("mem", user("mem")),
        ("keeper", user("keeper")),
        ("boss", admin("boss")),
        ("the machine owner", Scope::Unrestricted),
    ] {
        let listed = engine
            .list_domains(&ListDomainsParams::default(), &scope)
            .await
            .unwrap();
        assert_eq!(
            private_flag(&listed, "lab"),
            Some(true),
            "{who} sees `lab` marked private: {listed}"
        );
        assert_eq!(
            private_flag(&listed, "open"),
            Some(false),
            "{who} sees `open` marked shared: {listed}"
        );
    }
}

/// The `private` flag of one row of a domain listing, or `None` when the
/// listing does not name that domain at all.
fn private_flag(listed: &serde_json::Value, domain: &str) -> Option<bool> {
    listed
        .get("domains")?
        .as_array()?
        .iter()
        .find(|row| row.get("name").and_then(|n| n.as_str()) == Some(domain))?
        .get("private")?
        .as_bool()
}

/// An invitation is what makes the difference, not the shape of the request:
/// the member and the owner read exactly what the stranger cannot - and so does
/// an instance admin, who is never a member of anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_and_the_owner_see_the_private_domain() {
    let (_tmp, engine) = fixture().await;
    for (account, scope) in [
        ("mem", user("mem")),
        ("keeper", user("keeper")),
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

const OWNER_RULE: &str = "only the owner of 'lab' or an instance admin may change `read_only`";

fn lab_manifest(tmp: &tempfile::TempDir) -> String {
    std::fs::read_to_string(tmp.path().join("lab/MANIFEST.md")).unwrap()
}

fn sha256(text: &str) -> String {
    crate::support::sha256_hex(text.as_bytes())
}

fn switch_on(text: &str) -> String {
    text.replacen("\n---\n", "\nread_only: on\n---\n", 1)
}

/// A text edit of the MANIFEST that writes `read_only: on` into it. Not
/// `set_frontmatter`: that operation sets lifecycle keys only.
fn edit_switching_on(domain: &str) -> crystalline_service::params::EditParams {
    crystalline_service::params::EditParams {
        identifier: "manifest".into(),
        domain: domain.into(),
        operation: "find_replace".into(),
        find_text: Some("recorded_at: 2026-01-01\n".into()),
        content: Some("recorded_at: 2026-01-01\nread_only: on\n".into()),
        ..Default::default()
    }
}

/// A private domain that is also read-only answers a stranger as a domain
/// nobody registered: the refusal sentence would tell them it exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_private_read_only_domain_is_unknown_to_a_stranger() {
    let (tmp, engine) = fixture().await;
    std::fs::write(
        tmp.path().join("lab/MANIFEST.md"),
        switch_on(&lab_manifest(&tmp)),
    )
    .unwrap();
    let params = crystalline_service::params::EditParams {
        identifier: "lab-note".into(),
        domain: "lab".into(),
        operation: "append".into(),
        content: Some("- [fact] x #confidential".into()),
        ..Default::default()
    };
    let stranger = engine
        .edit_engram_as(&params, None, &user("out"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            stranger,
            crystalline_service::EngineError::UnknownDomain { .. }
        ),
        "{stranger:?}"
    );
    let keeper = engine
        .edit_engram_as(&params, None, &user("keeper"))
        .await
        .unwrap_err();
    assert!(
        matches!(keeper, crystalline_service::EngineError::DomainReadOnly(_)),
        "{keeper:?}"
    );
}

/// Only the owner or an instance admin switches it off: a member is refused
/// with the owner rule's own words.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_owner_or_an_admin_switches_read_only_off() {
    let (tmp, engine) = fixture().await;
    std::fs::write(
        tmp.path().join("lab/MANIFEST.md"),
        switch_on(&lab_manifest(&tmp)),
    )
    .unwrap();
    let off = [("read_only".to_string(), "off".to_string())];
    for member in ["mem", "ed"] {
        let refused = engine
            .set_manifest_policies("lab", &off, &user(member))
            .await
            .unwrap_err();
        assert!(
            matches!(refused, crystalline_service::EngineError::Forbidden(_)),
            "{member}: {refused:?}"
        );
        assert_eq!(refused.to_string(), OWNER_RULE);
    }
    engine
        .set_manifest_policies("lab", &off, &user("keeper"))
        .await
        .unwrap();
    assert!(!engine.domain_is_read_only("lab").await);
}

/// Switching read_only on is the owner's, like switching it off (decision
/// D10): an editor of the domain is refused on every path that writes the
/// MANIFEST, with the owner rule's own sentence, and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_editor_cannot_switch_read_only_on_by_any_path() {
    let (tmp, engine) = fixture().await;
    let ed = user("ed");
    let before = lab_manifest(&tmp);
    let refused = |err: crystalline_service::EngineError| {
        assert!(
            matches!(err, crystalline_service::EngineError::Forbidden(_)),
            "{err:?}"
        );
        assert_eq!(err.to_string(), OWNER_RULE);
    };

    // The policies card and REST PATCH.
    let on = [("read_only".to_string(), "on".to_string())];
    refused(
        engine
            .set_manifest_policies("lab", &on, &ed)
            .await
            .unwrap_err(),
    );

    // edit_engram on the MANIFEST.
    refused(
        engine
            .edit_engram_as(&edit_switching_on("lab"), None, &ed)
            .await
            .unwrap_err(),
    );

    // A whole-text save of the MANIFEST as an engram (REST PUT on the
    // `manifest` permalink, the engram editor).
    let save = crystalline_service::params::SaveParams {
        domain: "lab".into(),
        identifier: "manifest".into(),
        content: switch_on(&before),
        expected_checksum: sha256(&before),
    };
    refused(engine.save_engram(&save, &ed).await.unwrap_err());

    // The MANIFEST editor's own verb, now that it carries the scope.
    refused(
        engine
            .save_manifest("lab", &switch_on(&before), &sha256(&before), &ed, false)
            .await
            .unwrap_err(),
    );

    // A restore of the MANIFEST.
    refused(
        engine
            .restore_engram("lab", "MANIFEST.md", &switch_on(&before), &ed)
            .await
            .unwrap_err(),
    );

    assert_eq!(lab_manifest(&tmp), before, "the MANIFEST is byte-identical");
    assert!(!engine.domain_is_read_only("lab").await);

    // The editor still edits the MANIFEST: only the switch-on is the owner's.
    let other = crystalline_service::params::EditParams {
        identifier: "manifest".into(),
        domain: "lab".into(),
        operation: "set_frontmatter".into(),
        key: Some("status".into()),
        value: Some("current".into()),
        ..Default::default()
    };
    engine
        .edit_engram_as(&other, None, &ed)
        .await
        .expect("an ordinary MANIFEST edit lands");

    // A shared domain has no owning account: an instance editor holds
    // `Write` there, so only an admin switches it, and `out` is refused.
    let out = user("out");
    let open_rule = "only the owner of 'open' or an instance admin may change `read_only`";
    let err = engine
        .set_manifest_policies("open", &on, &out)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), open_rule);
    let err = engine
        .edit_engram_as(&edit_switching_on("open"), None, &out)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), open_rule);
    assert!(!engine.domain_is_read_only("open").await);
}

/// A move onto a missing MANIFEST.md writes the moved text as the MANIFEST,
/// so it meets the switch-on rule like every other MANIFEST writer (decision
/// D10): an instance editor on a shared domain is refused with the owner
/// rule, from the same domain and from another one, and nothing moves. A
/// move that leaves the flag off lands, and an instance admin may switch it
/// on this way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_onto_the_manifest_cannot_switch_read_only_on_for_an_editor() {
    let (tmp, engine) = fixture().await;
    let open = tmp.path().join("open");
    let lab = tmp.path().join("lab");
    let note = |permalink: &str, flag: bool| {
        let text = OPEN_NOTE.replacen(
            "permalink: open-note",
            &format!("permalink: {permalink}"),
            1,
        );
        match flag {
            true => switch_on(&text),
            false => text,
        }
    };
    std::fs::remove_file(open.join("MANIFEST.md")).unwrap();
    std::fs::write(open.join("locker.md"), note("locker", true)).unwrap();
    std::fs::write(open.join("plain.md"), note("plain", false)).unwrap();
    std::fs::write(lab.join("lab-locker.md"), note("lab-locker", true)).unwrap();
    engine.sync(None).await.unwrap();
    let open_rule = "only the owner of 'open' or an instance admin may change `read_only`";
    let onto_manifest = |identifier: &str, domain: &str| MoveParams {
        identifier: identifier.into(),
        domain: domain.into(),
        destination: "MANIFEST.md".into(),
        destination_domain: Some("open".into()),
        permalink: None,
        update_links: None,
    };

    // The same domain: `out` is an instance editor there.
    let err = engine
        .move_engram(&onto_manifest("locker", "open"), &user("out"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Forbidden(_)),
        "{err:?}"
    );
    assert_eq!(err.to_string(), open_rule);
    // Another domain: `ed` edits `lab` and is an instance editor on `open`.
    let err = engine
        .move_engram(&onto_manifest("lab-locker", "lab"), &user("ed"))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), open_rule, "{err:?}");
    assert!(!open.join("MANIFEST.md").exists(), "nothing moved");
    assert!(open.join("locker.md").exists());
    assert!(lab.join("lab-locker.md").exists());
    assert!(!engine.domain_is_read_only("open").await);

    // A move that leaves the flag off is not this rule's business.
    engine
        .move_engram(&onto_manifest("plain", "open"), &user("out"))
        .await
        .expect("a MANIFEST without the lock lands");
    assert!(open.join("MANIFEST.md").exists());
    assert!(!engine.domain_is_read_only("open").await);

    // An instance admin may switch it on by the same move.
    std::fs::remove_file(open.join("MANIFEST.md")).unwrap();
    engine.sync(None).await.unwrap();
    engine
        .move_engram(&onto_manifest("locker", "open"), &admin("boss"))
        .await
        .expect("the admin switches the lock on");
    assert!(engine.domain_is_read_only("open").await);
}

/// The same rule in lowercase: on a case-insensitive filesystem (the macOS
/// and Windows defaults) a move onto `manifest.md` lands as the file routing
/// reads as `MANIFEST.md`, so it is the MANIFEST and meets the switch-on rule
/// (decision D10) like the exact spelling. An instance editor is refused and
/// nothing moves; a move that leaves the flag off lands as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_onto_a_lowercase_manifest_cannot_switch_read_only_on_for_an_editor() {
    let (tmp, engine) = fixture().await;
    let open = tmp.path().join("open");
    let note = |permalink: &str, flag: bool| {
        let text = OPEN_NOTE.replacen(
            "permalink: open-note",
            &format!("permalink: {permalink}"),
            1,
        );
        match flag {
            true => switch_on(&text),
            false => text,
        }
    };
    std::fs::remove_file(open.join("MANIFEST.md")).unwrap();
    std::fs::write(open.join("locker.md"), note("locker", true)).unwrap();
    std::fs::write(open.join("plain.md"), note("plain", false)).unwrap();
    engine.sync(None).await.unwrap();
    let snapshot = |dir: &std::path::Path| {
        let mut files = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
        files
    };
    let onto_manifest = |identifier: &str| MoveParams {
        identifier: identifier.into(),
        domain: "open".into(),
        destination: "manifest.md".into(),
        destination_domain: Some("open".into()),
        permalink: None,
        update_links: None,
    };

    let before = snapshot(&open);
    let err = engine
        .move_engram(&onto_manifest("locker"), &user("out"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Forbidden(_)),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "only the owner of 'open' or an instance admin may change `read_only`"
    );
    assert_eq!(snapshot(&open), before, "nothing moved");
    assert!(!engine.domain_is_read_only("open").await);

    // A move that leaves the flag off is not this rule's business.
    engine
        .move_engram(&onto_manifest("plain"), &user("out"))
        .await
        .expect("a lowercase MANIFEST without the lock lands");
    assert!(!open.join("plain.md").exists());
    assert!(!engine.domain_is_read_only("open").await);
}

/// The owner and an instance admin switch it on, through the policies and
/// through the MANIFEST text; a viewer cannot, by either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_and_an_admin_switch_read_only_on_and_a_viewer_cannot() {
    let (tmp, engine) = fixture().await;
    let on = [("read_only".to_string(), "on".to_string())];
    let off = [("read_only".to_string(), "off".to_string())];

    // The viewer: refused by the policies path and by the text path.
    let viewer = user("mem");
    let refused = engine
        .set_manifest_policies("lab", &on, &viewer)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, crystalline_service::EngineError::Forbidden(_)),
        "{refused:?}"
    );
    let refused = engine
        .edit_engram_as(&edit_switching_on("lab"), None, &viewer)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, crystalline_service::EngineError::Forbidden(_)),
        "{refused:?}"
    );
    assert!(!engine.domain_is_read_only("lab").await);

    // The owner, through the policies and through the MANIFEST text.
    engine
        .set_manifest_policies("lab", &on, &user("keeper"))
        .await
        .unwrap();
    assert!(engine.domain_is_read_only("lab").await);
    engine
        .set_manifest_policies("lab", &off, &user("keeper"))
        .await
        .unwrap();
    let back_on = crystalline_service::params::EditParams {
        find_text: Some("read_only: off\n".into()),
        content: Some("read_only: on\n".into()),
        ..edit_switching_on("lab")
    };
    engine
        .edit_engram_as(&back_on, None, &user("keeper"))
        .await
        .unwrap();
    assert!(engine.domain_is_read_only("lab").await);
    engine
        .set_manifest_policies("lab", &off, &user("keeper"))
        .await
        .unwrap();

    // An instance admin, through the policies and through the MANIFEST editor.
    engine
        .set_manifest_policies("lab", &on, &admin("boss"))
        .await
        .unwrap();
    assert!(engine.domain_is_read_only("lab").await);
    engine
        .set_manifest_policies("lab", &off, &admin("boss"))
        .await
        .unwrap();
    let now = lab_manifest(&tmp);
    let switched = now.replace("read_only: off", "read_only: on");
    engine
        .save_manifest("lab", &switched, &sha256(&now), &admin("boss"), false)
        .await
        .unwrap();
    assert!(engine.domain_is_read_only("lab").await);
}

/// A capture titled after a virtual domain's MANIFEST replaces it (Ruling
/// I2), so it meets the switch-on rule like every other MANIFEST writer: an
/// editor who does not own the domain cannot lock it this way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_over_a_virtual_manifest_cannot_switch_read_only_on() {
    let (_tmp, engine) = fixture().await;
    engine.domain_add_virtual("scratch").await.unwrap();
    let p = crystalline_service::params::WriteParams {
        path: None,
        base64: None,
        domain: "scratch".into(),
        title: "MANIFEST".into(),
        content: Some("## Scope\n\n- x\n\n## When to Use\n\n- y\n".into()),
        folder: None,
        engram_type: Some("manifest".into()),
        tags: vec!["manifest".into()],
        status: None,
        metadata: Some(serde_json::json!({ "read_only": "on" })),
        overwrite: true,
        model: None,
        share_link: None,
    };
    let err = engine
        .write_engram_as(&p, None, &user("out"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Forbidden(_)),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "only the owner of 'scratch' or an instance admin may change `read_only`"
    );
    assert!(!engine.domain_is_read_only("scratch").await);
}

/// The switch-on rule refuses only a change of the flag: an editor who does
/// not own the domain still rewrites a virtual MANIFEST that leaves it off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capture_over_a_virtual_manifest_that_leaves_the_flag_alone_lands() {
    let (_tmp, engine) = fixture().await;
    engine.domain_add_virtual("scratch").await.unwrap();
    let p = crystalline_service::params::WriteParams {
        path: None,
        base64: None,
        domain: "scratch".into(),
        title: "MANIFEST".into(),
        content: Some("## Scope\n\n- x\n\n## When to Use\n\n- y\n".into()),
        folder: None,
        engram_type: Some("manifest".into()),
        tags: vec!["manifest".into()],
        status: None,
        metadata: None,
        overwrite: true,
        model: None,
        share_link: None,
    };
    engine
        .write_engram_as(&p, None, &user("out"))
        .await
        .expect("a MANIFEST that leaves read_only alone lands");
    assert!(!engine.domain_is_read_only("scratch").await);
}

/// A member who does not own a domain that is already locked is still refused
/// when they set `read_only: on` again: the key is the owner's whatever its
/// current value, so a non-owner cannot even re-state it. The MANIFEST is
/// byte-identical afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_owner_setting_read_only_on_a_locked_domain_gets_the_owner_sentence() {
    let (tmp, engine) = fixture().await;
    std::fs::write(
        tmp.path().join("lab/MANIFEST.md"),
        switch_on(&lab_manifest(&tmp)),
    )
    .unwrap();
    let before = lab_manifest(&tmp);
    assert!(engine.domain_is_read_only("lab").await);
    let on = [("read_only".to_string(), "on".to_string())];
    for member in ["mem", "ed"] {
        let refused = engine
            .set_manifest_policies("lab", &on, &user(member))
            .await
            .unwrap_err();
        assert!(
            matches!(refused, crystalline_service::EngineError::Forbidden(_)),
            "{member}: {refused:?}"
        );
        assert_eq!(refused.to_string(), OWNER_RULE);
    }
    assert_eq!(lab_manifest(&tmp), before, "the MANIFEST is byte-identical");
    assert!(engine.domain_is_read_only("lab").await);
}

/// B-1 item 1: in a reviewing domain a move onto the MANIFEST writes the
/// mover's draft, so the draft's text is what meets the switch-on rule. A
/// draft-only engram is found (no "unreadable" error), a non-owner whose draft
/// carries `read_only: on` is refused, and a draft that drops the base file's
/// flag is not refused for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reviewing_draft_move_onto_the_manifest_is_judged_by_the_draft() {
    let (tmp, engine) = fixture_with(&["open"]).await;
    let open = tmp.path().join("open");
    std::fs::remove_file(open.join("MANIFEST.md")).unwrap();
    // A base file that carries the flag, for the third leg.
    std::fs::write(
        open.join("flagged.md"),
        switch_on(
            &OPEN_NOTE
                .replacen("permalink: open-note", "permalink: flagged", 1)
                .replace("Open Note", "Flagged Note"),
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    // A draft-only engram of `out`'s that switches the lock on.
    let draft = crystalline_service::params::WriteParams {
        path: None,
        base64: None,
        domain: "open".into(),
        title: "Locker".into(),
        content: Some("## Scope\n\n- x\n\n## When to Use\n\n- y\n".into()),
        folder: None,
        engram_type: Some("manifest".into()),
        tags: vec!["manifest".into()],
        status: None,
        metadata: Some(serde_json::json!({ "read_only": "on" })),
        overwrite: false,
        model: None,
        share_link: None,
    };
    let written = engine
        .write_engram_as(&draft, None, &user("out"))
        .await
        .unwrap();
    assert_eq!(written["draft"], true, "{written}");

    let onto_manifest = |identifier: &str| MoveParams {
        identifier: identifier.into(),
        domain: "open".into(),
        destination: "MANIFEST.md".into(),
        destination_domain: None,
        permalink: None,
        update_links: None,
    };
    let err = engine
        .move_engram(&onto_manifest("locker"), &user("out"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Forbidden(_)),
        "the draft is found and judged: {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "only the owner of 'open' or an instance admin may change `read_only`"
    );
    assert!(!engine.domain_is_read_only("open").await);

    // Third leg: `out`'s draft of `flagged` drops the flag, so the move is
    // judged by that draft and the lock rule has nothing to refuse.
    let unflag = crystalline_service::params::EditParams {
        identifier: "flagged".into(),
        domain: "open".into(),
        operation: "find_replace".into(),
        find_text: Some("recorded_at: 2026-01-02\nread_only: on\n".into()),
        content: Some("recorded_at: 2026-01-02\n".into()),
        ..Default::default()
    };
    let edited = engine
        .edit_engram_as(&unflag, None, &user("out"))
        .await
        .unwrap();
    assert_eq!(edited["draft"], true, "{edited}");
    let moved = engine
        .move_engram(&onto_manifest("flagged"), &user("out"))
        .await
        .expect("a draft without the lock moves onto the MANIFEST");
    assert_eq!(moved["draft"], true, "{moved}");
    assert!(!engine.domain_is_read_only("open").await);
}

/// In a reviewing domain an engram the mover deleted in their own view (a
/// tombstone) is not there for them to move onto the MANIFEST: the plain
/// miss, and nothing is written. The resolve answers it; the draft read's own
/// miss behind it is the same sentence and is reached only by a race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_of_an_engram_deleted_in_review_is_not_found() {
    let (tmp, engine) = fixture_with(&["open"]).await;
    let open = tmp.path().join("open");
    std::fs::remove_file(open.join("MANIFEST.md")).unwrap();
    let onto_manifest = |identifier: &str| MoveParams {
        identifier: identifier.into(),
        domain: "open".into(),
        destination: "MANIFEST.md".into(),
        destination_domain: None,
        permalink: None,
        update_links: None,
    };
    std::fs::write(
        open.join("gone.md"),
        switch_on(
            &OPEN_NOTE
                .replacen("permalink: open-note", "permalink: gone", 1)
                .replace("Open Note", "Gone Note"),
        ),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let deleted = engine
        .delete_engram_as(
            &crystalline_service::params::DeleteParams {
                identifier: "gone".into(),
                domain: "open".into(),
                expected_checksum: None,
            },
            None,
            &user("out"),
        )
        .await
        .unwrap();
    assert_eq!(deleted["draft"], true, "{deleted}");
    let err = engine
        .move_engram(&onto_manifest("gone"), &user("out"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::NotFound(_)),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "no engram 'gone' in domain 'open'");
    assert!(!open.join("MANIFEST.md").exists());
    assert!(!engine.domain_is_read_only("open").await);
}
