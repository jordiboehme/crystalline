//! What an index row whose domain nobody registered is worth: nothing.
//!
//! Removing a domain used to leave its rows in the index on purpose and tell
//! the reader to run a full reindex to be rid of them. Until they did, every
//! unscoped search went on ranking a deliberately removed domain's content
//! above the knowledge they still keep, and the only remedy offered was the
//! heaviest command in the tool.
//!
//! The rule that ends that is one line: a row for a domain this instance has no
//! registration for is not a hit, not a count and not a facet value. It is the
//! rule a *named* read has always followed - a caller naming a domain nobody
//! registered gets nothing - said about a caller who named no domain at all, so
//! the two agree. It is a filter and never a deletion: every assertion here
//! holds with the rows still sitting in the index, which is what makes it safe
//! to ship on its own and what makes collecting them later a matter of disk
//! rather than of behaviour.
//!
//! The caller throughout is [`Scope::Unrestricted`], the machine owner, because
//! that is who felt this. The privacy screen is the same set and is pinned by
//! `tests/visibility.rs`; nothing here needs an accounts database.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::params::{
    BrowseParams, ContextParams, EvolveParams, ListDomainsParams, ReadParams, RecentParams,
    SearchParams, VocabularyParams,
};
use crystalline_service::{Engine, Scope};
use tokio::sync::Mutex;

const KEEP_MANIFEST: &str = "---\ntype: manifest\ntitle: keep\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# keep\n\n## Scope\n\n- The domain still registered\n\n## When to Use\n\n- Route here for current work\n";
const GONE_MANIFEST: &str = "---\ntype: manifest\ntitle: gone\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# gone\n\n## Scope\n\n- The domain that was removed\n\n## When to Use\n\n- Route here for retired work\n";
/// The engram that must keep answering. `protocol` is the word both notes
/// share, so a keyword search reaches for both and only this one comes back.
const KEEP_NOTE: &str = "---\ntype: engram\ntitle: Keep Note\npermalink: keep-note\ntags:\n  - current\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Keep Note\n\n- [decision] the protocol we use today #current\n";
/// The engram whose domain is removed under it. Its relation points into
/// `keep`, so it is also an inbound reference and a graph neighbour of the
/// engram above - the two surfaces where a dead domain surfaces without anybody
/// searching for it.
const GONE_NOTE: &str = "---\ntype: dossier\ntitle: Gone Note\npermalink: gone-note\ntags:\n  - retiredteam\nstatus: stable\nrecorded_at: 2026-01-03\n---\n\n# Gone Note\n\n- [legacy] the protocol we abandoned #retiredteam\n- relates_to [[keep:Keep Note]]\n";

/// One domain folder with a MANIFEST and one engram.
fn write_domain(
    root: &std::path::Path,
    name: &str,
    manifest: &str,
    note: &str,
) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), manifest).unwrap();
    std::fs::write(dir.join(format!("{name}-note.md")), note).unwrap();
    dir
}

/// The 0.17.0 shape, reproduced exactly: two domains indexed, then one of them
/// unregistered without its rows being touched, and a fresh engine opened over
/// the same index - which is what a daemon restart after a removal is.
///
/// The config is rewritten on disk as well as in memory, because
/// `Engine::refresh_domain` re-reads the file whenever a name misses the
/// snapshot: a removal that only forgot in memory would resurrect itself the
/// first time anything named it.
async fn fixture() -> (tempfile::TempDir, Arc<Engine>, Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let keep = write_domain(&root, "keep", KEEP_MANIFEST, KEEP_NOTE);
    let gone = write_domain(&root, "gone", GONE_MANIFEST, GONE_NOTE);

    let config_path = root.join("config.yaml");
    let mut both = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    both.domains
        .insert("keep".to_string(), DomainEntry::file(keep));
    both.domains
        .insert("gone".to_string(), DomainEntry::file(gone));
    crystalline_core::config::save_yaml(&config_path, &both).unwrap();

    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let indexed = Engine::new(store.clone(), both.clone(), None, Some(config_path.clone()));
    indexed.sync(None).await.unwrap();
    drop(indexed);

    // The removal: the registration goes, the rows stay.
    let mut only_keep = both.clone();
    only_keep.domains.shift_remove("gone");
    crystalline_core::config::save_yaml(&config_path, &only_keep).unwrap();

    let engine = Arc::new(Engine::new(
        store.clone(),
        only_keep,
        None,
        Some(config_path),
    ));
    (tmp, engine, store)
}

/// The same instance with `keep` and nothing else: one domain registered, one
/// domain indexed, no orphan anywhere.
///
/// The control for [`an_orphan_changes_nothing_about_what_is_served`]. Screening
/// something out moves several reads off their unfiltered fast path and onto a
/// narrowed one - the vocabulary sweep in particular stops being one store query
/// and becomes one per domain, merged - and those two paths have to produce the
/// same bytes, or an orphan in the index would quietly change the answer for
/// reads that have nothing to do with it.
async fn clean_fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let keep = write_domain(&root, "keep", KEEP_MANIFEST, KEEP_NOTE);

    let config_path = root.join("config.yaml");
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert("keep".to_string(), DomainEntry::file(keep));
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Arc::new(Engine::new(store, cfg, None, Some(config_path)));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

fn read(identifier: &str, domain: Option<&str>) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: domain.map(str::to_string),
    }
}

fn keyword(query: &str) -> SearchParams {
    SearchParams {
        query: Some(query.to_string()),
        ..SearchParams::default()
    }
}

/// The reported symptom, and the assertion that the rows really are still there
/// while it holds: a filter, not a deletion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unregistered_domain_is_not_a_hit_and_not_a_count() {
    let (_tmp, engine, store) = fixture().await;

    let hits = engine
        .search_engrams(&keyword("protocol"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        hits["total"], 1,
        "the total counts what is served, not what is stored: {hits}"
    );
    assert!(
        hits.to_string().contains("keep-note") && !hits.to_string().contains("gone"),
        "the removed domain is not an answer: {hits}"
    );

    // Nothing was deleted to make that true. The rows the search declined to
    // count are exactly where the removal left them.
    let stats = store.lock().await.domain_stats().await.unwrap();
    let gone = stats
        .iter()
        .find(|d| d.name == "gone")
        .expect("the removed domain still has its row");
    assert!(
        gone.engrams >= 2,
        "its engrams are still in the index: {gone:?}"
    );
}

/// Naming the removed domain is not a way back in, and it answers exactly as
/// naming a domain nobody ever registered does - the rule this extends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naming_an_unregistered_domain_answers_like_naming_an_unknown_one() {
    let (_tmp, engine, _store) = fixture().await;

    let named_gone = engine
        .search_engrams(
            &SearchParams {
                domains: vec!["gone".to_string()],
                ..keyword("protocol")
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let named_unknown = engine
        .search_engrams(
            &SearchParams {
                domains: vec!["nope".to_string()],
                ..keyword("protocol")
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        named_gone, named_unknown,
        "the whole envelope matches, not just the count: {named_gone}"
    );
    assert_eq!(named_gone["total"], 0, "{named_gone}");

    // The visible half of a mixed filter still answers, so this narrows rather
    // than refuses.
    let mixed = engine
        .search_engrams(
            &SearchParams {
                domains: vec!["gone".to_string(), "keep".to_string()],
                ..keyword("protocol")
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        mixed.to_string().contains("keep-note") && !mixed.to_string().contains("gone-note"),
        "{mixed}"
    );

    // And browsing it is the unregistered-domain error, which it already was.
    let browsed = engine
        .browse_domain(
            &BrowseParams {
                domain: "gone".to_string(),
                path: None,
                depth: None,
                glob: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        browsed.contains("gone") && browsed.contains("keep"),
        "it names the domain asked for and the registered set: {browsed}"
    );
}

/// The facet surface. Vocabulary is where a dead domain's tags, types and
/// observation categories go on being offered as the vocabulary to write in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_vocabulary_sweep_leaves_out_an_unregistered_domain() {
    let (_tmp, engine, _store) = fixture().await;

    let all = engine
        .vocabulary(&VocabularyParams { domain: None }, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        all.to_string().contains("current"),
        "the registered domain's vocabulary is there: {all}"
    );
    assert!(
        !all.to_string().contains("retiredteam") && !all.to_string().contains("legacy"),
        "the removed domain's tags and categories are not: {all}"
    );

    let named = engine
        .vocabulary(
            &VocabularyParams {
                domain: Some("gone".to_string()),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let unknown = engine
        .vocabulary(
            &VocabularyParams {
                domain: Some("nope".to_string()),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        named.to_string().replace("gone", "nope"),
        unknown.to_string(),
        "naming it reports the empty vocabulary an unknown domain reports: {named}"
    );
}

/// The activity feed, whose row limit is enforced in SQL - so a filter applied
/// to its answer rather than to its query would hand back short pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_activity_leaves_out_an_unregistered_domain() {
    let (_tmp, engine, _store) = fixture().await;
    let recent = engine
        .recent_activity(
            &RecentParams {
                domains: Vec::new(),
                timeframe: Some("100y".to_string()),
                types: Vec::new(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        recent.to_string().contains("keep-note") && !recent.to_string().contains("gone-note"),
        "the feed covers what is served: {recent}"
    );
}

/// Two cross-domain surfaces that surface a dead domain without anybody
/// searching for it: who points here, and what is next to here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_reference_or_neighbour_reaches_out_of_an_unregistered_domain() {
    let (_tmp, engine, _store) = fixture().await;

    let payload = engine
        .read_engram(&read("keep-note", Some("keep")), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        payload.get("inbound").is_none(),
        "with its only referrer unregistered, the inbound block is omitted: {payload}"
    );

    let inbound = engine
        .inbound_references(
            &read("keep-note", Some("keep")),
            None,
            None,
            None,
            None,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        inbound["total"], 0,
        "and the paged verb agrees with the count it draws from: {inbound}"
    );
    assert!(!inbound.to_string().contains("gone"), "{inbound}");

    let context = engine
        .build_context(
            &ContextParams {
                anchor: "crystalline://keep/keep-note".to_string(),
                depth: Some(2),
                domains: Vec::new(),
                timeframe: None,
                max_related: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        !context.to_string().contains("gone"),
        "the neighbour in the removed domain is cut out of the slice: {context}"
    );
    assert_eq!(
        context["edges"].as_array().map(|e| e.len()),
        Some(0),
        "and so is the edge that would have pointed at it: {context}"
    );

    let graph = engine
        .graph_neighborhood("crystalline://keep/keep-note", 2, 100, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        !graph.to_string().contains("gone"),
        "the graph view answers the same: {graph}"
    );

    // An anchor inside the removed domain is an anchor that is not there.
    let anchored = engine
        .build_context(
            &ContextParams {
                anchor: "crystalline://gone/gone-note".to_string(),
                depth: Some(2),
                domains: Vec::new(),
                timeframe: None,
                max_related: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    let absent = engine
        .build_context(
            &ContextParams {
                anchor: "crystalline://nope/gone-note".to_string(),
                depth: Some(2),
                domains: Vec::new(),
                timeframe: None,
                max_related: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert_eq!(
        anchored.to_string(),
        absent.to_string().replace("nope", "gone"),
        "an anchor in the removed domain reads as one that was never written"
    );
}

/// The identifier grammar's cross-domain half: a bare permalink is resolved
/// against every domain at once, so it is a door into a removed one that never
/// mentions its name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bare_identifier_never_resolves_into_an_unregistered_domain() {
    let (_tmp, engine, _store) = fixture().await;
    let err = engine
        .read_engram(&read("gone-note", None), &Scope::Unrestricted)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !err.contains("gone/gone-note"),
        "it does not resolve, and the refusal does not quote a location in it: {err}"
    );
    assert!(
        engine
            .read_engram(&read("keep-note", None), &Scope::Unrestricted)
            .await
            .is_ok(),
        "the registered domain still resolves bare identifiers"
    );
}

/// The three surfaces an agent is routed and maintained by. All three build
/// from the registrations rather than from the index, so this pins that they
/// still do rather than fixing something that was broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_listing_the_routing_block_and_the_sweep_leave_out_an_unregistered_domain() {
    let (_tmp, engine, _store) = fixture().await;

    let listed = engine
        .list_domains(&ListDomainsParams::default(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        listed.to_string().contains("keep") && !listed.to_string().contains("gone"),
        "the index an agent routes by names what is served: {listed}"
    );

    let routing = engine
        .routing_text_scoped(&Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        routing.contains("keep") && !routing.contains("retired work"),
        "no routing line and no MANIFEST bullet for it: {routing}"
    );

    let swept = engine
        .evolve_engrams(&EvolveParams::default(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        swept["scope"]["domains"],
        serde_json::json!(["keep"]),
        "the maintenance sweep does not tend a domain nobody registered: {swept}"
    );
    assert!(!swept.to_string().contains("gone-note"), "{swept}");
}

/// The other direction, and the one this could plausibly have broken: a domain
/// registered *after* this engine started is served, not screened out as
/// unregistered.
///
/// `domain add` writes the config file and then asks the running daemon to sync
/// the new domain by name, which is what teaches the engine about it. This
/// walks that sequence: the sweep that follows must find it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_registered_after_the_engine_started_is_served() {
    let (tmp, engine, _store) = fixture().await;
    let root = tmp.path().to_path_buf();
    let later = write_domain(
        &root,
        "later",
        &GONE_MANIFEST.replace("gone", "later"),
        &GONE_NOTE
            .replace("Gone Note", "Later Note")
            .replace("permalink: gone-note", "permalink: later-note"),
    );

    let config_path = root.join("config.yaml");
    let mut cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    cfg.domains
        .insert("later".to_string(), DomainEntry::file(later));
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    engine.sync(Some("later")).await.unwrap();

    let hits = engine
        .search_engrams(&keyword("protocol"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        hits.to_string().contains("later-note"),
        "a registration made after startup is served by an unnamed sweep: {hits}"
    );
    assert!(
        !hits.to_string().contains("gone-note"),
        "and the removed one still is not: {hits}"
    );
}

/// An orphan in the index changes nothing at all about the domain that is
/// served - not one field of one envelope.
///
/// Worth its own test because the screen is what decides whether a read takes
/// its unfiltered fast path or a narrowed one, and those are not always the same
/// query. The vocabulary sweep is the sharp case: with nothing screened out it
/// is a single all-domain store query, and with something screened out it
/// becomes one query per served domain merged back together. Two shapes for one
/// answer is how they drift, so this pins them equal, field for field, against
/// an instance that has `keep` and has never heard of `gone`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_orphan_changes_nothing_about_what_is_served() {
    let (_tmp, orphaned, _store) = fixture().await;
    let (_clean_tmp, clean) = clean_fixture().await;

    for (what, a, b) in [
        (
            "the vocabulary sweep",
            orphaned
                .vocabulary(&VocabularyParams { domain: None }, &Scope::Unrestricted)
                .await
                .unwrap(),
            clean
                .vocabulary(&VocabularyParams { domain: None }, &Scope::Unrestricted)
                .await
                .unwrap(),
        ),
        (
            "an unfiltered search",
            orphaned
                .search_engrams(&keyword("protocol"), &Scope::Unrestricted)
                .await
                .unwrap(),
            clean
                .search_engrams(&keyword("protocol"), &Scope::Unrestricted)
                .await
                .unwrap(),
        ),
        (
            "a search with no query at all",
            orphaned
                .search_engrams(&SearchParams::default(), &Scope::Unrestricted)
                .await
                .unwrap(),
            clean
                .search_engrams(&SearchParams::default(), &Scope::Unrestricted)
                .await
                .unwrap(),
        ),
        (
            "the activity feed",
            orphaned
                .recent_activity(
                    &RecentParams {
                        domains: Vec::new(),
                        timeframe: Some("100y".to_string()),
                        types: Vec::new(),
                    },
                    &Scope::Unrestricted,
                )
                .await
                .unwrap(),
            clean
                .recent_activity(
                    &RecentParams {
                        domains: Vec::new(),
                        timeframe: Some("100y".to_string()),
                        types: Vec::new(),
                    },
                    &Scope::Unrestricted,
                )
                .await
                .unwrap(),
        ),
    ] {
        assert_eq!(a, b, "{what} answers the same with an orphan in the index");
    }
}

/// `total` is the count the query reports, not the length of the page it
/// returned, so a filter that narrowed the rows and left the count alone would
/// show here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_total_is_the_query_count_and_not_the_page_length() {
    let (_tmp, engine, _store) = fixture().await;
    let hits = engine
        .search_engrams(
            &SearchParams {
                limit: Some(1),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let total = hits["total"].as_i64().expect("a total");
    assert_eq!(
        hits["count"], 1,
        "the page is capped at the limit asked for: {hits}"
    );
    assert!(
        total > 1,
        "and the total counts past it, so it is the query's own count: {hits}"
    );
    assert!(
        !hits.to_string().contains("gone"),
        "with the removed domain in neither: {hits}"
    );

    // The same total, unpaged, is exactly what the served domain holds - two
    // engrams, the MANIFEST and the note - and not the four the index stores.
    let all = engine
        .search_engrams(&SearchParams::default(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        all["total"], total,
        "paging does not change the total: {all}"
    );
    assert_eq!(
        all["total"], 2,
        "the served domain's engrams, not the index's: {all}"
    );
}
