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
    let indexed = Engine::new(store.clone(), both.clone(), None, Some(config_path.clone()))
        .with_state_dir(root.join("state"));
    indexed.sync(None).await.unwrap();
    drop(indexed);

    // The removal: the registration goes, the rows stay.
    let mut only_keep = both.clone();
    only_keep.domains.shift_remove("gone");
    crystalline_core::config::save_yaml(&config_path, &only_keep).unwrap();

    let engine = Arc::new(
        Engine::new(store.clone(), only_keep, None, Some(config_path))
            // A collection sweeps the overlay journal under the state
            // directory, and that sweep removes a folder tree: without this
            // every test here would be deleting under the developer's own.
            .with_state_dir(root.join("state")),
    );
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
    let engine = Arc::new(
        Engine::new(store, cfg, None, Some(config_path)).with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

fn read(identifier: &str, domain: Option<&str>) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: domain.map(str::to_string),
        share_link: None,
    }
}

/// The domain of every row in one array-valued field of an envelope, so an
/// assertion can name the field that carries the claim instead of scanning the
/// whole envelope for a string. Panics when the field is not an array of rows
/// with a domain on them, because an assertion that quietly found nothing to
/// check is worse than no assertion.
fn domains_in(envelope: &serde_json::Value, field: &str) -> Vec<String> {
    envelope[field]
        .as_array()
        .unwrap_or_else(|| panic!("'{field}' is an array here: {envelope}"))
        .iter()
        .map(|row| {
            row["domain"]
                .as_str()
                .unwrap_or_else(|| panic!("every row in '{field}' names its domain: {envelope}"))
                .to_string()
        })
        .collect()
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
    assert_eq!(
        domains_in(&hits, "hits"),
        vec!["keep".to_string()],
        "the one hit is the registered domain's, and the removed domain has none: {hits}"
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
    assert_eq!(
        domains_in(&recent, "engrams"),
        vec!["keep".to_string(); 2],
        "the feed covers what is served and nothing else: {recent}"
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
    assert_eq!(
        inbound["hits"],
        serde_json::json!([]),
        "no referrer out of it is listed: {inbound}"
    );
    assert_eq!(
        inbound["types"],
        serde_json::json!([]),
        "and its relation is not in the per-relation summary either, which is \
         counted by its own query: {inbound}"
    );

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
    assert_eq!(
        domains_in(&context, "nodes"),
        vec!["keep".to_string()],
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
    assert_eq!(
        domains_in(&graph, "nodes"),
        vec!["keep".to_string()],
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
/// the new domain by name. This walks that sequence, and the sweep that follows
/// must find it. What makes it servable is the registration in the file, which
/// the screen resolves either way - see
/// [`a_domain_the_config_file_gains_is_registered_and_served`], the same
/// question with the by-name sync taken out - so what the named sync adds here
/// is the sync target and the watch rather than the right to be served.
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

/// The tag verbs are a door into a removed domain's content that never names
/// it: `crystalline tags merge` with no `--domain` prechecks against the whole
/// index, so a tag only the removed domain carries still counts as a tag that
/// exists.
///
/// It must not. A merge has to land on a tag something served carries, which is
/// exactly what merging into a tag nobody ever wrote is refused for, so the two
/// refusals are the same refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merge_cannot_land_on_a_tag_only_an_unregistered_domain_carries() {
    let (_tmp, engine, _store) = fixture().await;

    let into_the_orphans_tag = engine
        .retag("current", "retiredteam", None, true, false, false)
        .await
        .unwrap_err()
        .to_string();
    let into_a_tag_nobody_wrote = engine
        .retag("current", "never-written", None, true, false, false)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        into_the_orphans_tag,
        into_a_tag_nobody_wrote.replace("never-written", "retiredteam"),
        "a tag only the removed domain carries is a tag that is not there: {into_the_orphans_tag}"
    );
}

/// The other half of the same door, and the one that writes: a rename with no
/// `--domain` used to list the removed domain's engrams by name and path and
/// then rewrite them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_neither_lists_nor_rewrites_an_unregistered_domains_engrams() {
    let (tmp, engine, store) = fixture().await;
    let note = tmp.path().join("gone").join("gone-note.md");
    let before = store
        .lock()
        .await
        .domain_stats()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.name == "gone")
        .expect("the removed domain still has its row");

    let renamed = engine
        .retag("retiredteam", "archived", None, false, false, false)
        .await
        .unwrap();
    assert_eq!(
        renamed["engrams"],
        serde_json::json!([]),
        "nothing in the removed domain is listed: {renamed}"
    );
    assert_eq!(
        renamed["rewritten"], 0,
        "and nothing in it is rewritten: {renamed}"
    );
    assert!(
        std::fs::read_to_string(&note)
            .unwrap()
            .contains("retiredteam"),
        "its file on disk still carries the tag"
    );

    // Its rows are where the removal left them, down to every count.
    let after = store
        .lock()
        .await
        .domain_stats()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.name == "gone")
        .expect("the removed domain still has its row");
    assert_eq!(before, after, "not one row of it moved");

    // And the registered domain is still renamed, so this narrows rather than
    // refuses.
    let served = engine
        .retag("current", "archived", None, false, false, false)
        .await
        .unwrap();
    assert_eq!(
        served["rewritten"], 1,
        "the served domain's engram is renamed: {served}"
    );
}

/// The third tier of a named lookup, which an unnamed sweep has to resolve
/// through too: a domain the configuration *file* gains while this engine runs,
/// which nothing has ever named here.
///
/// `Engine::domain_entry` re-reads the file for a name it does not know, so a
/// named read has always answered for such a domain. An unnamed sweep screening
/// it out as unregistered would be the same named-versus-unnamed disagreement
/// this rule exists to end, pointing the other way - and a collector keyed on
/// the same set would delete a registered domain's rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_the_config_file_gains_is_registered_and_served() {
    let (tmp, engine, store) = fixture().await;
    let root = tmp.path().to_path_buf();
    let later = write_domain(
        &root,
        "later",
        &GONE_MANIFEST.replace("gone", "later"),
        &GONE_NOTE
            .replace("Gone Note", "Later Note")
            .replace("permalink: gone-note", "permalink: later-note"),
    );

    // Its rows are indexed by a throwaway engine over the same store, so the
    // engine under test is never told the name by anything but the file.
    let config_path = root.join("config.yaml");
    let mut with_later: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    with_later
        .domains
        .insert("later".to_string(), DomainEntry::file(later));
    let indexer = Engine::new(
        store.clone(),
        with_later.clone(),
        None,
        Some(config_path.clone()),
    )
    .with_state_dir(root.join("state"));
    indexer.sync(Some("later")).await.unwrap();
    drop(indexer);

    // The control, and it is the rule: rows with no registration anywhere are
    // not served and the name is not registered.
    assert!(
        !engine.registered_domain_names().contains("later"),
        "rows alone are not a registration"
    );
    let before = engine
        .search_engrams(&keyword("protocol"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        !before.to_string().contains("later-note"),
        "nor are they served: {before}"
    );

    // The registration, written to the file and nowhere else.
    crystalline_core::config::save_yaml(&config_path, &with_later).unwrap();

    let registered = engine.registered_domain_names();
    assert!(
        registered.contains("later") && registered.contains("keep"),
        "the file's registrations are registrations: {registered:?}"
    );
    assert!(
        !registered.contains("gone"),
        "and the removed domain is still not one: {registered:?}"
    );

    let hits = engine
        .search_engrams(&keyword("protocol"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        hits.to_string().contains("later-note"),
        "an unnamed sweep answers for it, as a named read always has: {hits}"
    );
    assert!(
        !hits.to_string().contains("gone-note"),
        "and still not for the removed one: {hits}"
    );
}

// --- collection after the grace period ---------------------------------------
//
// Part A stops serving an orphan's rows; it never removes one. This is the
// half that removes them, and it is the only code in this file that deletes
// anything, so every test below asserts what SURVIVED as well as what went.
//
// The rule in one line: a domain the configuration does not name, whose stamp
// says it has been gone longer than the grace period, loses its engram rows and
// keeps its domain row. Absence from the configuration is necessary and never
// sufficient - the stamp has to be stale too - and a configuration that could
// not be read proves no absence at all.

/// Stamp `name` as last seen registered `ago` before now, the way a daemon
/// sweep that ran then would have left it. Returns the instant written.
async fn plant_stamp(store: &Arc<Mutex<dyn Store>>, name: &str, ago: chrono::Duration) -> String {
    let when = (chrono::Utc::now() - ago).to_rfc3339();
    store
        .lock()
        .await
        .stamp_registered(&[name], &when)
        .await
        .unwrap();
    when
}

/// The engram count the index holds for `name`, and `None` when the domain has
/// no row at all - the difference between "cleared" and "gone", which
/// collection must never blur.
async fn engrams_of(store: &Arc<Mutex<dyn Store>>, name: &str) -> Option<i64> {
    store
        .lock()
        .await
        .domain_stats()
        .await
        .unwrap()
        .iter()
        .find(|d| d.name == name)
        .map(|d| d.engrams)
}

/// The `last_registered` stamp the index holds for `name`.
async fn stamp_of(store: &Arc<Mutex<dyn Store>>, name: &str) -> Option<String> {
    store
        .lock()
        .await
        .domain_stats()
        .await
        .unwrap()
        .iter()
        .find(|d| d.name == name)
        .and_then(|d| d.last_registered.clone())
}

/// The names in a report's `collected` list.
fn collected(report: &serde_json::Value) -> Vec<String> {
    report["collected"]
        .as_array()
        .unwrap_or_else(|| panic!("'collected' is an array here: {report}"))
        .iter()
        .map(|n| {
            n.as_str()
                .expect("a collected name is a string")
                .to_string()
        })
        .collect()
}

/// The `considered` row for one domain, or `None` when the sweep did not
/// consider it.
fn considered<'a>(report: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    report["considered"]
        .as_array()
        .unwrap_or_else(|| panic!("'considered' is an array here: {report}"))
        .iter()
        .find(|row| row["domain"] == name)
}

/// The case the accumulation defect is: rows whose domain nobody has named in
/// the configuration for longer than the grace period, and which nothing short
/// of a full reindex would ever have removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_unregistered_past_the_grace_period_is_collected() {
    let (_tmp, engine, store) = fixture().await;
    plant_stamp(&store, "gone", chrono::Duration::days(13)).await;
    let before = engrams_of(&store, "gone").await.unwrap();
    assert!(before >= 2, "the orphan starts with its rows: {before}");

    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert_eq!(
        collected(&report),
        vec!["gone".to_string()],
        "the stale orphan is the one collected: {report}"
    );
    assert_eq!(
        report["engrams_removed"], before,
        "and the report counts the rows it removed: {report}"
    );
    let row = considered(&report, "gone").expect("the orphan is reported");
    assert_eq!(row["collected"], true, "reported as collected: {report}");
    assert!(
        row["age_days"].as_i64().unwrap() >= 13,
        "with the age that justified it: {report}"
    );

    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(0),
        "its engram rows are gone and its domain row is not"
    );
    assert!(
        engrams_of(&store, "keep").await.unwrap() >= 2,
        "the registered domain is untouched"
    );
}

/// Absence from the configuration is necessary and never sufficient: a domain
/// unregistered by hand at noon does not cost a resync by evening.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_unregistered_for_an_hour_is_not_collected() {
    let (_tmp, engine, store) = fixture().await;
    plant_stamp(&store, "gone", chrono::Duration::hours(1)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert!(
        collected(&report).is_empty(),
        "an hour is not a week: {report}"
    );
    let row = considered(&report, "gone").expect("it is still reported as considered");
    assert_eq!(row["collected"], false, "and reported as kept: {report}");
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "its rows are all still there"
    );
}

/// A registered domain is never a candidate, whatever its stamp says - and the
/// stamp is refreshed before anything is considered, which is what makes a week
/// of a machine being off, or of a read-only instance, safe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_registered_domain_is_never_collected_whatever_its_stamp() {
    let (_tmp, engine, store) = fixture().await;
    let ancient = plant_stamp(&store, "keep", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "keep").await.unwrap();

    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert!(
        !collected(&report).contains(&"keep".to_string()),
        "a registered domain is not collected: {report}"
    );
    assert!(
        considered(&report, "keep").is_none(),
        "it is not even a candidate: {report}"
    );
    assert_eq!(
        engrams_of(&store, "keep").await,
        Some(before),
        "and it keeps every row"
    );
    let now = stamp_of(&store, "keep").await.expect("it is stamped");
    assert_ne!(
        now, ancient,
        "the sweep stamped it before it considered anything"
    );
}

/// `None` means never stamped, not stamped infinitely long ago. Every row an
/// upgrade inherits reads `None`, so the first sweep after one starts the clock
/// and collects nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_never_stamped_domain_is_stamped_now_and_not_collected() {
    let (_tmp, engine, store) = fixture().await;
    assert_eq!(
        stamp_of(&store, "gone").await,
        None,
        "the inherited row carries no stamp"
    );
    let before = engrams_of(&store, "gone").await.unwrap();

    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert!(
        collected(&report).is_empty(),
        "no evidence of age is no licence to delete: {report}"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "its rows survive the sweep"
    );
    assert!(
        stamp_of(&store, "gone").await.is_some(),
        "and it leaves the sweep with a clock running, or it would be immortal"
    );
}

/// A dry run answers the question and writes nothing at all: not a removal, not
/// a stamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dry_run_reports_the_same_set_and_removes_nothing() {
    let (_tmp, engine, store) = fixture().await;
    plant_stamp(&store, "gone", chrono::Duration::days(13)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let dry = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), true)
        .await
        .unwrap();
    assert_eq!(dry["dry_run"], true, "the report says which it was: {dry}");
    assert_eq!(
        collected(&dry),
        vec!["gone".to_string()],
        "it names what a real run would collect: {dry}"
    );
    assert_eq!(
        dry["engrams_removed"], 0,
        "and removed nothing to say it: {dry}"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "the rows are where they were"
    );
    assert_eq!(
        stamp_of(&store, "keep").await,
        None,
        "and a preview did not stamp the registered domain either"
    );

    let wet = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();
    assert_eq!(
        collected(&wet),
        collected(&dry),
        "the real run collects the set the preview named: {wet}"
    );
    assert_eq!(wet["engrams_removed"], before, "this time for real: {wet}");
    assert_eq!(engrams_of(&store, "gone").await, Some(0));
}

/// A read-only instance collects nothing and says so, rather than refusing: a
/// caller asking what is collectable still gets the answer.
///
/// It does stamp, though, and that is not a detail. Stamping is index
/// maintenance rather than a content write, and on a shared database a
/// read-only instance that never stamped would watch a peer's sweep age out and
/// collect the very domains it is serving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_collects_nothing_and_says_so() {
    let (tmp, _engine, store) = fixture().await;
    let config_path = tmp.path().join("config.yaml");
    let cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let read_only = Engine::new(store.clone(), cfg, None, Some(config_path))
        .with_read_only(true)
        .with_state_dir(tmp.path().join("state"));
    plant_stamp(&store, "gone", chrono::Duration::days(13)).await;
    let ancient = plant_stamp(&store, "keep", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let report = read_only
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert_eq!(report["read_only"], true, "it says which it is: {report}");
    assert!(
        report["skipped"]
            .as_str()
            .is_some_and(|s| s.contains("read-only") && s.contains("were stamped")),
        "in words, with the reason, and saying what this run did do: {report}"
    );
    assert!(
        report["stamped"].as_u64().is_some_and(|n| n > 0),
        "which is stamp the domains it serves: {report}"
    );
    assert!(
        collected(&report).is_empty(),
        "and collects nothing: {report}"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "the rows are untouched"
    );
    assert_ne!(
        stamp_of(&store, "keep").await.expect("it is stamped"),
        ancient,
        "and the domains it serves were defended: their stamp moved"
    );
}
/// The same instance asked to look rather than to act. A read-only preview
/// writes nothing at all - not a removal and not a stamp - so the sentence it
/// hands a reader may not claim the stamp the real run makes. This is the
/// mirror of the sentence above, and the two are the only two shapes there
/// are.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_preview_says_nothing_was_changed() {
    let (tmp, _engine, store) = fixture().await;
    let config_path = tmp.path().join("config.yaml");
    let cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let read_only = Engine::new(store.clone(), cfg, None, Some(config_path))
        .with_read_only(true)
        .with_state_dir(tmp.path().join("state"));
    let ancient = plant_stamp(&store, "keep", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let report = read_only
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), true)
        .await
        .unwrap();

    assert_eq!(report["read_only"], true, "{report}");
    assert_eq!(
        report["stamped"], 0,
        "a preview writes nothing, so nothing was stamped: {report}"
    );
    let skipped = report["skipped"]
        .as_str()
        .unwrap_or_else(|| panic!("a read-only run says so: {report}"));
    assert!(
        skipped.contains("read-only") && skipped.contains("nothing was changed"),
        "and the sentence says what this run did: {report}"
    );
    assert!(
        !skipped.contains("were stamped"),
        "never a stamp this run did not make: {report}"
    );
    assert_eq!(
        stamp_of(&store, "keep").await.expect("it is stamped"),
        ancient,
        "the stamp really did not move"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "and no row was touched"
    );
}

/// The configuration is the evidence of absence. When it cannot be read there
/// is no such evidence, so the sweep establishes no registered set, collects
/// nothing, stamps nothing and names the reason - both when the file is
/// unparseable and when it is not there at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_configuration_collects_nothing() {
    let (tmp, engine, store) = fixture().await;
    let config_path = tmp.path().join("config.yaml");
    plant_stamp(&store, "gone", chrono::Duration::days(13)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    for (case, prepare) in [
        ("unparseable", 0u8),
        // Absent counts too: a configuration that is not there is
        // indistinguishable from one in which every domain was just removed.
        ("absent", 1u8),
    ] {
        if prepare == 0 {
            std::fs::write(&config_path, "domains: [this is not: yaml\n  - at all\n").unwrap();
        } else {
            std::fs::remove_file(&config_path).unwrap();
        }

        let report = engine
            .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
            .await
            .unwrap();

        assert!(
            collected(&report).is_empty(),
            "{case}: an unreadable configuration proves no absence: {report}"
        );
        assert!(
            report["skipped"]
                .as_str()
                .is_some_and(|s| s.contains("configuration")),
            "{case}: and the report names it: {report}"
        );
        assert_eq!(
            engrams_of(&store, "gone").await,
            Some(before),
            "{case}: every row survives"
        );
        assert_eq!(
            stamp_of(&store, "keep").await,
            None,
            "{case}: and nothing was stamped, since nothing was known to be registered"
        );

        // A person asking waives the grace period and nothing else. Without a
        // readable configuration there is still no domain shown absent from
        // one, so the answer is the same refusal.
        let asked = engine.collect_orphaned_domains(None, false).await.unwrap();
        assert!(
            collected(&asked).is_empty(),
            "{case}: asking does not make an unreadable file evidence: {asked}"
        );
        assert!(
            asked["skipped"]
                .as_str()
                .is_some_and(|s| s.contains("configuration")),
            "{case}: and it says so on that path too: {asked}"
        );
        assert_eq!(engrams_of(&store, "gone").await, Some(before));
    }
}

/// A virtual domain's rows are not a derived copy of anything: they are the
/// knowledge. `domain_remove` already refuses to drop them without an explicit
/// purge, and this answers nobody's confirmation, so it reports one - with the
/// `kept` word a doctor names the purge path from - and collects it never. On
/// the sweep's path and on the person's alike: asking is not confirming.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_virtual_domains_rows_are_never_collected() {
    let (_tmp, engine, store) = fixture().await;
    // The orphan, with its kind flipped in the index: rows whose only copy is
    // the database, and no registration anywhere.
    store
        .lock()
        .await
        .upsert_domain("gone", None, crystalline_core::config::DomainKind::Virtual)
        .await
        .unwrap();
    plant_stamp(&store, "gone", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "gone").await.unwrap();
    assert!(before >= 2, "it has rows to lose: {before}");

    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert!(
        collected(&report).is_empty(),
        "no age makes a virtual domain collectable: {report}"
    );
    let row = considered(&report, "gone").expect("it is reported rather than hidden");
    assert_eq!(row["kind"], "virtual", "named as what it is: {report}");
    assert_eq!(row["collected"], false);
    assert_eq!(
        row["kept"], "virtual",
        "and kept for the one reason a doctor must word differently: {report}"
    );
    assert_eq!(
        row["engrams"], before,
        "with the rows at stake counted: {report}"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "and its only copy is still there"
    );

    // A person asking does not make it collectable either: the confirmation a
    // virtual domain needs is `domain remove --purge`, not this.
    let asked = engine.collect_orphaned_domains(None, false).await.unwrap();
    assert!(
        collected(&asked).is_empty(),
        "nor does asking for it: {asked}"
    );
    assert_eq!(
        considered(&asked, "gone").expect("still reported")["kept"],
        "virtual"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "and the rows are still all there"
    );
}

/// A read-only instance meets the same virtual orphan, and says the same
/// thing about it.
///
/// Read-only and virtual both keep every row, so which word the report
/// carries changes nothing about what happens - it changes what a person is
/// told to do. "This instance is read-only" points at a writable instance that
/// would collect these rows, and no instance ever will: the rows are the
/// domain's only copy everywhere. So virtual wins, and the doctor names the
/// one command that ends a virtual domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_virtual_orphan_is_virtual_even_on_a_read_only_instance() {
    let (tmp, _engine, store) = fixture().await;
    let config_path = tmp.path().join("config.yaml");
    let cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    let read_only = Engine::new(store.clone(), cfg, None, Some(config_path))
        .with_read_only(true)
        .with_state_dir(tmp.path().join("state"));
    store
        .lock()
        .await
        .upsert_domain("gone", None, crystalline_core::config::DomainKind::Virtual)
        .await
        .unwrap();
    plant_stamp(&store, "gone", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let report = read_only
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    let row = considered(&report, "gone").expect("it is reported rather than hidden");
    assert_eq!(
        row["kept"], "virtual",
        "the reason that names a remedy wins over the one that names this instance: {report}"
    );
    assert_eq!(row["collected"], false);
    assert!(
        collected(&report).is_empty(),
        "and a read-only instance still collects nothing: {report}"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "with its only copy untouched"
    );
}

/// The on-demand path, and the one rule it waives. A person asking is the
/// signal the grace period exists to wait for, so `None` waits for nothing: a
/// never-stamped orphan - which is every row an index inherits from a version
/// that stranded them - is collected on first contact rather than a week after.
///
/// The daemon's own sweep, over the same domain in the same state, keeps it and
/// starts its clock. Both halves are asserted here because the pair is the
/// whole decision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_person_asking_collects_a_never_stamped_orphan_and_a_sweep_does_not() {
    let (_tmp, engine, store) = fixture().await;
    assert_eq!(
        stamp_of(&store, "gone").await,
        None,
        "the inherited row carries no stamp"
    );
    let before = engrams_of(&store, "gone").await.unwrap();

    // The sweep first, with no grace period at all: an unstamped domain is not
    // old, it is unmeasured, so even a zero grace keeps it.
    let swept = engine
        .collect_orphaned_domains(Some(chrono::Duration::zero()), false)
        .await
        .unwrap();
    assert!(
        collected(&swept).is_empty(),
        "no stamp is no evidence, whatever the grace period: {swept}"
    );
    assert_eq!(
        considered(&swept, "gone").expect("reported")["kept"],
        "unstamped",
        "and the report says which rule kept it: {swept}"
    );
    assert!(
        stamp_of(&store, "gone").await.is_some(),
        "the sweep started its clock"
    );
    assert_eq!(engrams_of(&store, "gone").await, Some(before));

    // Then the person, whose asking is what the waiting was for.
    let asked = engine.collect_orphaned_domains(None, false).await.unwrap();
    assert_eq!(
        asked["on_demand"], true,
        "the report says which path it was: {asked}"
    );
    assert_eq!(asked["grace_seconds"], serde_json::Value::Null);
    assert_eq!(
        collected(&asked),
        vec!["gone".to_string()],
        "asked for, and collected: {asked}"
    );
    assert_eq!(asked["engrams_removed"], before);
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(0),
        "its rows are gone and its domain row is not"
    );
    assert!(
        engrams_of(&store, "keep").await.unwrap() >= 2,
        "and the registered domain is untouched"
    );
}

/// The question a person asks before they ask for it: the same set, and not one
/// row removed to answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_on_demand_dry_run_lists_the_orphan_and_removes_nothing() {
    let (_tmp, engine, store) = fixture().await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let dry = engine.collect_orphaned_domains(None, true).await.unwrap();
    assert_eq!(dry["dry_run"], true, "{dry}");
    assert_eq!(dry["on_demand"], true, "{dry}");
    assert_eq!(
        collected(&dry),
        vec!["gone".to_string()],
        "it names the never-stamped orphan a real ask would collect: {dry}"
    );
    assert_eq!(dry["engrams_removed"], 0, "and removed nothing: {dry}");
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "every row is where it was"
    );
    assert_eq!(
        stamp_of(&store, "gone").await,
        None,
        "and a preview did not stamp anything either"
    );

    let asked = engine.collect_orphaned_domains(None, false).await.unwrap();
    assert_eq!(
        collected(&asked),
        collected(&dry),
        "the ask collects the set the preview named: {asked}"
    );
    assert_eq!(engrams_of(&store, "gone").await, Some(0));
}

/// Give `name`'s domain row a host lock held by another instance, heartbeating
/// `ago` before now, the way a peer over a shared database leaves one. Returns
/// nothing: the lock is read back through `domain_stats` like any other column.
async fn plant_host_lock(
    store: &Arc<Mutex<dyn Store>>,
    name: &str,
    root: &std::path::Path,
    ago: chrono::Duration,
) {
    let store = store.lock().await;
    let id = store
        .upsert_domain(
            name,
            Some(&root.to_string_lossy()),
            crystalline_core::config::DomainKind::File,
        )
        .await
        .unwrap();
    let beat = (chrono::Utc::now() - ago).to_rfc3339();
    let stale_before = (chrono::Utc::now() - chrono::Duration::seconds(90)).to_rfc3339();
    store
        .claim_domain_host(id, "peer-instance", "a peer", &beat, &stale_before, false)
        .await
        .unwrap();
}

/// The single most dangerous line item in this plan, pinned rather than
/// read-verified: a domain registered in the configuration **file** and named
/// through this engine by nothing at all.
///
/// It is absent from the startup snapshot and absent from the discovered
/// overlay, so a collector keyed on those two tiers would see an orphan, and on
/// the person's path would delete a registered domain's rows on the spot. The
/// third tier - the file re-read - is the only thing between it and that, so
/// this test fails the moment anyone swaps the checked helper for
/// `known_domain_names()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_registered_only_in_the_config_file_is_stamped_and_never_collected() {
    let (tmp, engine, store) = fixture().await;
    let root = tmp.path().to_path_buf();
    let later = write_domain(
        &root,
        "later",
        &GONE_MANIFEST.replace("gone", "later"),
        &GONE_NOTE
            .replace("Gone Note", "Later Note")
            .replace("permalink: gone-note", "permalink: later-note"),
    );

    // Its rows land through a throwaway engine over the same store, so the
    // engine under test is never told the name by anything but the file.
    let config_path = root.join("config.yaml");
    let mut with_later: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    with_later
        .domains
        .insert("later".to_string(), DomainEntry::file(later));
    let indexer = Engine::new(
        store.clone(),
        with_later.clone(),
        None,
        Some(config_path.clone()),
    )
    .with_state_dir(root.join("state"));
    indexer.sync(Some("later")).await.unwrap();
    drop(indexer);
    crystalline_core::config::save_yaml(&config_path, &with_later).unwrap();

    // As stale as a stamp gets. Only the registration saves it.
    let ancient = plant_stamp(&store, "later", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "later").await.unwrap();
    assert!(before >= 2, "it has rows to lose: {before}");

    for (label, grace) in [
        ("the sweep", Some(chrono::Duration::days(7))),
        ("the person", None),
    ] {
        let report = engine.collect_orphaned_domains(grace, false).await.unwrap();
        assert!(
            !collected(&report).contains(&"later".to_string()),
            "{label}: a domain the file registers is not collected: {report}"
        );
        assert!(
            considered(&report, "later").is_none(),
            "{label}: it is not even a candidate: {report}"
        );
        assert_eq!(
            engrams_of(&store, "later").await,
            Some(before),
            "{label}: and it keeps every row"
        );
    }
    assert_ne!(
        stamp_of(&store, "later").await.expect("it is stamped"),
        ancient,
        "the sweep stamped it as the registration it is"
    );
}

/// A shared database: several instances registering different domains against
/// one index. A domain this instance has no registration for may be another
/// instance's current work, and another instance's live registration is a
/// registration - so a live host lock keeps the rows on the sweep's path and on
/// the person's alike, because neither of them is the peer that would know.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_a_live_peer_hosts_is_kept_on_both_paths() {
    let (tmp, engine, store) = fixture().await;
    plant_host_lock(
        &store,
        "gone",
        &tmp.path().join("gone"),
        chrono::Duration::seconds(5),
    )
    .await;
    plant_stamp(&store, "gone", chrono::Duration::days(400)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    for (label, grace) in [
        ("the sweep", Some(chrono::Duration::days(7))),
        ("the person", None),
    ] {
        let report = engine.collect_orphaned_domains(grace, false).await.unwrap();
        assert!(
            collected(&report).is_empty(),
            "{label}: a live peer's domain is not this instance's to collect: {report}"
        );
        assert_eq!(
            considered(&report, "gone").expect("it is reported")["kept"],
            "hosted_elsewhere",
            "{label}: and the report says which rule kept it: {report}"
        );
        assert_eq!(
            engrams_of(&store, "gone").await,
            Some(before),
            "{label}: every row survives"
        );
    }
}

/// The other half of the same rule: a host lock nobody has heartbeated within
/// the stale threshold is a lock a claim would take over, so it defends
/// nothing. With no registration here either, the usual rules apply and the
/// rows go.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_whose_host_lock_went_stale_is_collected() {
    let (tmp, engine, store) = fixture().await;
    plant_host_lock(
        &store,
        "gone",
        &tmp.path().join("gone"),
        chrono::Duration::days(1),
    )
    .await;
    plant_stamp(&store, "gone", chrono::Duration::days(400)).await;
    assert!(engrams_of(&store, "gone").await.unwrap() >= 2);

    let report = engine
        .collect_orphaned_domains(Some(chrono::Duration::days(7)), false)
        .await
        .unwrap();

    assert_eq!(
        collected(&report),
        vec!["gone".to_string()],
        "a dead peer's lock keeps nothing alive: {report}"
    );
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(0),
        "its rows are gone and its domain row is not"
    );
}

// --- the daemon's own sweep --------------------------------------------------
//
// The collector above is the what; this is the when. The daemon runs it on a
// timer beside its other interval tasks, with the grace period supplied, so an
// always-on instance tidies itself without anybody asking - and without ever
// taking the on-demand path, which would drop a row the grace period is still
// holding.

/// Wait until `f` holds, polling on a short interval, and panic after the
/// deadline with what it saw instead.
async fn within<F, Fut>(what: &str, f: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if f().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{what} did not happen within two seconds");
}

/// The accumulation defect answered by the daemon: nobody asked, and the rows
/// of a domain gone a fortnight are collected anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_sweep_collects_a_stale_orphan() {
    let (_tmp, engine, store) = fixture().await;
    plant_stamp(&store, "gone", chrono::Duration::days(13)).await;
    assert!(
        engrams_of(&store, "gone").await.unwrap() >= 2,
        "the orphan starts with its rows"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(crystalline_service::daemon::run_orphan_sweep(
        engine.clone(),
        std::time::Duration::from_millis(25),
        shutdown_rx,
    ));

    within("the sweep collects the stale orphan", || async {
        engrams_of(&store, "gone").await == Some(0)
    })
    .await;
    assert!(
        engrams_of(&store, "keep").await.unwrap() >= 2,
        "and the registered domain keeps every row"
    );

    // Shutdown mirrors the other interval tasks: the task exits promptly.
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), handle)
        .await
        .expect("the sweep task exits when shutdown is signaled")
        .unwrap();
}

/// The sweep supplies the grace period, and that is the whole difference
/// between it and a person asking: an hour of absence survives any number of
/// ticks. A sweep that passed the on-demand path would empty this domain on its
/// first one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_sweep_keeps_an_orphan_inside_the_grace_period() {
    let (_tmp, engine, store) = fixture().await;
    plant_stamp(&store, "gone", chrono::Duration::hours(1)).await;
    let before = engrams_of(&store, "gone").await.unwrap();

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(crystalline_service::daemon::run_orphan_sweep(
        engine.clone(),
        std::time::Duration::from_millis(25),
        shutdown_rx,
    ));

    // Several ticks, and the rows are all still there.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        engrams_of(&store, "gone").await,
        Some(before),
        "an hour is not a week, on any number of sweeps"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
}
