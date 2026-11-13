//! Relative Markdown links between engrams as graph edges: the index stores the
//! path a relative link lands on and binds it to the engram at that path.

use std::path::Path;

use crystalline_index::ReadStore;
use crystalline_index::{EdgeKind, Store, TursoStore, sync_domain};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// Run a body against Turso (always) and Postgres (when configured).
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                $body(&store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(scratch) = crate::pg::scratch("rl") {
                    let store = crystalline_index::PostgresStore::open_in_schema(
                        scratch.url(),
                        scratch.name(),
                    )
                    .await
                    .expect("open the postgres test schema");
                    $body(&store).await;
                }
            }
        }
    };
}

/// A converted collection: notes link each other by relative path, and the
/// target's explicit permalink is nothing like its path, so only the path can
/// bind the link.
async fn a_relative_link_resolves_to_the_engram_at_that_path(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "overview.md",
        &engram("Overview", "start-here", "The plant at a glance.\n"),
    );
    write(
        root,
        "pumps/p-12.md",
        &engram(
            "P-12",
            "kit/pump-twelve",
            "- part_of [Overview](../overview.md)\n\nSee [the overview](<../overview.md#top>) again.\n",
        ),
    );
    write(
        root,
        "pumps/list.md",
        &engram("Pumps", "pumps", "Pump [twelve](p-12.md) is the big one.\n"),
    );
    let report = sync_domain(store, "plant", root).await.unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    let stats = &store.domain_stats().await.unwrap()[0];
    assert_eq!((stats.relations, stats.unresolved_relations), (1, 0));
    assert_eq!((stats.links, stats.unresolved_links), (2, 0));

    let p12 = store
        .lookup_id("plant", "kit/pump-twelve")
        .await
        .unwrap()
        .unwrap();
    let overview = store
        .lookup_id("plant", "start-here")
        .await
        .unwrap()
        .unwrap();
    let pumps = store.lookup_id("plant", "pumps").await.unwrap().unwrap();
    let slice = store.neighbors(&[p12], 1, None).await.unwrap();
    assert!(
        slice.edges.iter().any(|e| e.from == p12
            && e.to == overview
            && e.kind == EdgeKind::Relation
            && e.rel_type == "part_of"),
        "the relation bullet is a typed edge: {:?}",
        slice.edges
    );
    assert!(
        slice
            .edges
            .iter()
            .any(|e| e.from == p12 && e.to == overview && e.kind == EdgeKind::Link),
        "the prose link is an untyped edge: {:?}",
        slice.edges
    );
    assert!(
        slice
            .edges
            .iter()
            .any(|e| e.from == pumps && e.to == p12 && e.kind == EdgeKind::Link),
        "a link in the same folder lands too: {:?}",
        slice.edges
    );

    // What the row says the link points at: the domain-relative path.
    let outbound = store.outbound_refs(p12, None).await.unwrap();
    let targets: Vec<&str> = outbound.iter().map(|o| o.to_target.as_str()).collect();
    assert_eq!(targets, ["overview.md", "overview.md"]);
    assert!(outbound.iter().all(|o| o.resolved));
}
parity!(
    a_relative_link_resolves_to_the_engram_at_that_path_on_both_backends,
    a_relative_link_resolves_to_the_engram_at_that_path
);

/// A link that climbs out of the domain names nothing in it: the row keeps the
/// target as written and stays unresolved, which is what V102 reads.
async fn a_relative_link_out_of_the_domain_stays_unresolved(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "overview.md",
        &engram(
            "Overview",
            "overview",
            "Out [there](../../elsewhere/overview.md).\n",
        ),
    );
    sync_domain(store, "plant", root).await.unwrap();
    let id = store.lookup_id("plant", "overview").await.unwrap().unwrap();
    let outbound = store.outbound_refs(id, None).await.unwrap();
    assert_eq!(outbound.len(), 1);
    assert_eq!(outbound[0].to_target, "../../elsewhere/overview.md");
    assert!(!outbound[0].resolved);
    let domain = store
        .upsert_domain(
            "plant",
            Some(&root.to_string_lossy()),
            crystalline_index::DomainKind::File,
        )
        .await
        .unwrap();
    let unresolved = store.unresolved_refs(domain, None).await.unwrap();
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
}
parity!(
    a_relative_link_out_of_the_domain_stays_unresolved_on_both_backends,
    a_relative_link_out_of_the_domain_stays_unresolved
);

/// A relative link into a file that appears later binds on the later sync, the
/// way a forward wikilink does.
async fn a_relative_link_binds_once_its_target_appears(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a.md",
        &engram("A", "a", "Next is [b](notes/b.md).\n"),
    );
    sync_domain(store, "d", root).await.unwrap();
    assert_eq!(store.domain_stats().await.unwrap()[0].unresolved_links, 1);
    write(root, "notes/b.md", &engram("B", "bee", "B body.\n"));
    let second = sync_domain(store, "d", root).await.unwrap();
    assert_eq!(second.links_resolved, 1);
    assert_eq!(store.domain_stats().await.unwrap()[0].unresolved_links, 0);
}
parity!(
    a_relative_link_binds_once_its_target_appears_on_both_backends,
    a_relative_link_binds_once_its_target_appears
);

/// No row records how its reference was written, so the path arm is open to a
/// wikilink too: `[[pumps/p-12.md]]` names that file's path and binds to it.
/// A valid permalink never ends in `.md`, so no wikilink that resolved before
/// resolves differently now.
async fn a_wikilink_spelled_as_a_path_binds_to_that_file(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pumps/p-12.md",
        &engram("P-12", "kit/pump-twelve", "Pump.\n"),
    );
    write(root, "a.md", &engram("A", "a", "See [[pumps/p-12.md]].\n"));
    sync_domain(store, "d", root).await.unwrap();
    assert_eq!(store.domain_stats().await.unwrap()[0].unresolved_links, 0);
}
parity!(
    a_wikilink_spelled_as_a_path_binds_to_that_file_on_both_backends,
    a_wikilink_spelled_as_a_path_binds_to_that_file
);

/// A file name with a non-ASCII letter binds whether the link spells it
/// percent-encoded (`b%C3%A4r.md`, the UTF-8 bytes) or raw (`bär.md`): both
/// decode to the same path. The match is byte exact, with no Unicode
/// normalization, so the decomposed spelling (`a` plus U+0308) of a name
/// stored composed names another file and stays unresolved.
async fn a_non_ascii_file_name_binds_encoded_or_raw(store: &dyn Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "notes/b\u{e4}r.md",
        &engram("B\u{e4}r", "baer", "The bear.\n"),
    );
    write(
        root,
        "a.md",
        &engram(
            "A",
            "a",
            "- part_of [B\u{e4}r](notes/b%C3%A4r.md)\n\n\
             See [raw](notes/b\u{e4}r.md#top) and [lower](notes/b%c3%a4r.md).\n\n\
             Not [decomposed](notes/ba\u{308}r.md).\n",
        ),
    );
    let report = sync_domain(store, "d", root).await.unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    let a = store.lookup_id("d", "a").await.unwrap().unwrap();
    let outbound = store.outbound_refs(a, None).await.unwrap();
    let rows: Vec<(&str, bool)> = outbound
        .iter()
        .map(|o| (o.to_target.as_str(), o.resolved))
        .collect();
    assert_eq!(
        rows,
        [
            ("notes/b\u{e4}r.md", true),
            ("notes/b\u{e4}r.md", true),
            ("notes/ba\u{308}r.md", false),
        ],
        "the relation and the line's one prose link resolve, the decomposed one does not"
    );
    let bear = store.lookup_id("d", "baer").await.unwrap().unwrap();
    let slice = store.neighbors(&[a], 1, None).await.unwrap();
    assert!(
        slice.edges.iter().any(|e| e.from == a
            && e.to == bear
            && e.kind == EdgeKind::Relation
            && e.rel_type == "part_of"),
        "{:?}",
        slice.edges
    );
    assert!(
        slice
            .edges
            .iter()
            .any(|e| e.from == a && e.to == bear && e.kind == EdgeKind::Link),
        "{:?}",
        slice.edges
    );
}
parity!(
    a_non_ascii_file_name_binds_encoded_or_raw_on_both_backends,
    a_non_ascii_file_name_binds_encoded_or_raw
);

/// The repo's fixture domains, synced once each: (name, links, unresolved
/// links, relations, unresolved relations).
async fn fixture_counts() -> Vec<(String, i64, i64, i64, i64)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let store = TursoStore::open_in_memory().await.unwrap();
    for name in ["canonical", "domain-good", "domain-bad", "import-source"] {
        sync_domain(&store, name, &root.join(name)).await.unwrap();
    }
    let mut out: Vec<_> = store
        .domain_stats()
        .await
        .unwrap()
        .into_iter()
        .map(|s| {
            (
                s.name,
                s.links,
                s.unresolved_links,
                s.relations,
                s.unresolved_relations,
            )
        })
        .collect();
    out.sort();
    out
}

/// The fixtures hold no relative note links, so reading them did not change
/// what they are: every wikilink and relation edge, resolved or not, is what
/// it was before relative links became edges (counted on the parser of
/// generation 2).
#[tokio::test]
async fn reindexing_the_fixtures_keeps_every_wikilink_edge() {
    let counts = fixture_counts().await;
    let expected = [
        ("canonical", 9, 6, 18, 9),
        ("domain-bad", 3, 2, 1, 0),
        ("domain-good", 0, 0, 2, 0),
        ("import-source", 0, 0, 0, 0),
    ]
    .map(|(n, a, b, c, d)| (n.to_string(), a, b, c, d));
    assert_eq!(counts, expected);
}
