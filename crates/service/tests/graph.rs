//! Engine-level tests for `graph_neighborhood`, the nodes-and-typed-edges view
//! the Fluid graph screen draws from.
//!
//! The fixture is a chain in one virtual domain, written through the engine so
//! the relations are resolved the way a real write resolves them:
//!
//! ```text
//! Alpha -supersedes-> Beta -relates_to-> Gamma -relates_to-> Delta -relates_to-> Epsilon
//! ```
//!
//! Anchored on Beta, the chain makes every rule observable: one hop reaches
//! Alpha and Gamma, a second reaches Delta, and nothing reaches Epsilon, so a
//! depth that was clamped is visible in the node count rather than in a field
//! the response would have to invent.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::{Engine, EngineError};
use crystalline_service::params::WriteParams;
use tokio::sync::Mutex;

/// An engine over a single virtual domain named `notes`.
async fn engine() -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    Engine::new(Arc::new(Mutex::new(store)), cfg, None, None)
}

/// Write one engram into `notes` with a body and a status.
async fn write(engine: &Engine, title: &str, status: &str, body: &str) {
    engine
        .write_engram(&WriteParams {
            domain: "notes".to_string(),
            title: title.to_string(),
            content: body.to_string(),
            folder: None,
            engram_type: Some("engram".to_string()),
            tags: vec!["chain".to_string()],
            status: Some(status.to_string()),
            metadata: None,
            overwrite: false,
        })
        .await
        .unwrap();
}

/// The chain above. Alpha is retired, so a neighborhood that includes it proves
/// the API reports retired knowledge rather than hiding it.
async fn chain() -> Engine {
    let engine = engine().await;
    write(
        &engine,
        "Alpha",
        "superseded",
        "The first link.\n\n- supersedes [[Beta]]\n",
    )
    .await;
    write(
        &engine,
        "Beta",
        "stable",
        "The second link.\n\n- relates_to [[Gamma]]\n",
    )
    .await;
    write(
        &engine,
        "Gamma",
        "stable",
        "The third link.\n\n- relates_to [[Delta]]\n",
    )
    .await;
    write(
        &engine,
        "Delta",
        "stable",
        "The fourth link.\n\n- relates_to [[Epsilon]]\n",
    )
    .await;
    write(&engine, "Epsilon", "stable", "The last link.\n").await;
    engine
}

/// The `permalink` of every node in a graph response, in the order returned.
fn permalinks(graph: &serde_json::Value) -> Vec<String> {
    graph["nodes"]
        .as_array()
        .expect("a graph response carries a nodes array")
        .iter()
        .map(|n| n["permalink"].as_str().unwrap().to_string())
        .collect()
}

/// One hop out of an anchor: the anchor itself, the engram that supersedes it
/// and the one it relates to, with both edges typed and directed.
#[tokio::test]
async fn depth_one_returns_the_anchor_its_neighbors_and_the_typed_edges() {
    let engine = chain().await;

    let graph = engine
        .graph_neighborhood("crystalline://notes/beta", 1, 100)
        .await
        .unwrap();

    let mut names = permalinks(&graph);
    names.sort();
    assert_eq!(
        names,
        vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
        "the anchor and its two neighbors: {graph}"
    );
    assert_eq!(
        graph["nodes"][0]["permalink"], "beta",
        "the anchor leads the list: {graph}"
    );
    assert_eq!(graph["truncated"], false, "nothing was cut: {graph}");

    // Every node carries what the graph view labels and styles it with.
    let anchor = &graph["nodes"][0];
    assert!(anchor["id"].as_i64().is_some(), "{graph}");
    assert_eq!(anchor["domain"], "notes");
    assert_eq!(anchor["title"], "Beta");
    assert_eq!(anchor["status"], "stable");
    assert_eq!(anchor["type"], "engram");

    // The edges keep their direction, so a client can tell a backlink from an
    // outbound reference without asking again.
    let edges = graph["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2, "one edge per hop: {graph}");
    let by_id = |permalink: &str| -> i64 {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["permalink"] == permalink)
            .unwrap_or_else(|| panic!("{permalink} is in the slice: {graph}"))["id"]
            .as_i64()
            .unwrap()
    };
    let (alpha, beta, gamma) = (by_id("alpha"), by_id("beta"), by_id("gamma"));
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == alpha && e["to"] == beta && e["rel_type"] == "supersedes"),
        "the inbound edge, pointing at the anchor: {graph}"
    );
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == beta && e["to"] == gamma && e["rel_type"] == "relates_to"),
        "the outbound edge, pointing away: {graph}"
    );
}

/// The depth is the server's to decide: below one it is one, above two it is
/// two, so a hand-written URL can neither ask for nothing nor walk the whole
/// index.
#[tokio::test]
async fn the_depth_is_clamped_to_one_hop_or_two() {
    let engine = chain().await;

    let count = async |depth: u8| -> usize {
        engine
            .graph_neighborhood("crystalline://notes/beta", depth, 100)
            .await
            .unwrap()["nodes"]
            .as_array()
            .unwrap()
            .len()
    };

    assert_eq!(count(1).await, 3, "alpha, beta, gamma");
    assert_eq!(count(0).await, 3, "zero hops is one hop");
    assert_eq!(count(2).await, 4, "delta joins on the second hop");
    assert_eq!(count(5).await, 4, "five hops is two: epsilon stays out");
}

/// The node cap is honest about itself: it cuts to the cap and says it cut, and
/// an edge whose other end was cut goes with it rather than dangling.
#[tokio::test]
async fn the_node_cap_cuts_and_reports_it() {
    let engine = chain().await;

    let capped = engine
        .graph_neighborhood("crystalline://notes/beta", 2, 1)
        .await
        .unwrap();
    assert_eq!(
        permalinks(&capped),
        vec!["beta".to_string()],
        "the anchor survives a cap of one: {capped}"
    );
    assert_eq!(
        capped["truncated"], true,
        "and the cut is reported: {capped}"
    );
    assert!(
        capped["edges"].as_array().unwrap().is_empty(),
        "an edge to a node that was cut is cut too: {capped}"
    );

    let whole = engine
        .graph_neighborhood("crystalline://notes/beta", 2, 100)
        .await
        .unwrap();
    assert_eq!(
        whole["truncated"], false,
        "a cap nothing reaches cuts nothing: {whole}"
    );
}

/// The cap has a server-side ceiling, so a client asking for the whole index in
/// one response is answered with a bounded slice that says it is one.
#[tokio::test]
async fn the_cap_has_a_server_side_ceiling() {
    let engine = engine().await;
    write(&engine, "Hub", "stable", "The center of the star.\n").await;
    for n in 0..200 {
        write(
            &engine,
            &format!("Spoke {n:03}"),
            "stable",
            "A spoke.\n\n- relates_to [[Hub]]\n",
        )
        .await;
    }

    let graph = engine
        .graph_neighborhood("crystalline://notes/hub", 1, 100_000)
        .await
        .unwrap();
    assert_eq!(
        graph["nodes"].as_array().unwrap().len(),
        150,
        "the ceiling holds whatever was asked for: {}",
        graph["nodes"]
    );
    assert_eq!(graph["truncated"], true);
}

/// Retired knowledge is part of the graph: the API reports it with its status,
/// and the client decides how to show it.
#[tokio::test]
async fn a_retired_neighbor_is_included_with_its_status() {
    let engine = chain().await;

    let graph = engine
        .graph_neighborhood("crystalline://notes/beta", 1, 100)
        .await
        .unwrap();
    let alpha = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["permalink"] == "alpha")
        .expect("the retired neighbor is in the slice");
    assert_eq!(
        alpha["status"], "superseded",
        "reported as retired rather than dropped: {graph}"
    );
}

/// One connection is one edge: an engram that both declares a `links_to`
/// relation and writes the wikilink in its prose relates to the target once, and
/// the picture says so once.
#[tokio::test]
async fn a_relation_and_its_prose_link_are_one_edge() {
    let engine = engine().await;
    write(&engine, "Target", "stable", "The target.\n").await;
    write(
        &engine,
        "Source",
        "stable",
        "Prose pointing at [[Target]].\n\n- links_to [[Target]]\n",
    )
    .await;

    let graph = engine
        .graph_neighborhood("crystalline://notes/target", 1, 100)
        .await
        .unwrap();
    let edges = graph["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "drawn once, not twice: {graph}");
    assert_eq!(edges[0]["rel_type"], "links_to");
}

/// An anchor with one live neighbor and one retired one, for the tests that
/// prove retired knowledge yields first when the cap bites. Beta (the
/// anchor, stable) relates to Gamma (stable) and Delta (deprecated).
async fn fixture_with_retired_neighbor() -> ((), Engine) {
    let engine = engine().await;
    write(
        &engine,
        "Beta",
        "stable",
        "The anchor.\n\n- relates_to [[Gamma]]\n- relates_to [[Delta]]\n",
    )
    .await;
    write(&engine, "Gamma", "stable", "A live neighbor.\n").await;
    write(&engine, "Delta", "deprecated", "A retired neighbor.\n").await;
    ((), engine)
}

/// Over the cap, retired knowledge yields first: the live neighborhood
/// survives and the payload counts what was cut. Under the cap (the sibling
/// tests) nothing changes - same node set, hidden stays zero.
#[tokio::test]
async fn the_cap_prunes_retired_nodes_first_and_counts_them() {
    // Fixture: an anchor with one live and one retired neighbor, capped to 2
    // (the anchor plus one). The survivor must be the live one.
    let (_tmp, engine) = fixture_with_retired_neighbor().await;
    let value = engine
        .graph_neighborhood("crystalline://notes/beta", 1, 2)
        .await
        .unwrap();
    let nodes = value["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert!(
        nodes.iter().all(|n| !crystalline_index::is_retired_status(
            n["status"].as_str().unwrap_or("")
        ) || n["permalink"] == "beta"),
        "a retired non-anchor node survived while a live one was cut: {value}"
    );
    assert_eq!(value["truncated"], true);
    assert!(value["hidden"].as_u64().unwrap() >= 1);
}

/// Under the cap the count is an honest zero.
#[tokio::test]
async fn an_uncapped_neighborhood_hides_nothing() {
    let (_tmp, engine) = fixture_with_retired_neighbor().await;
    let value = engine
        .graph_neighborhood("crystalline://notes/beta", 1, 100)
        .await
        .unwrap();
    assert_eq!(value["hidden"], 0);
    assert_eq!(value["truncated"], false);
}

/// The two ways an anchor can be wrong are two different errors, so the HTTP
/// surface above can map them apart.
#[tokio::test]
async fn a_bad_anchor_is_refused_by_kind() {
    let engine = chain().await;

    let malformed = engine
        .graph_neighborhood("beta", 1, 100)
        .await
        .expect_err("an anchor that is not a URL is refused");
    assert!(
        matches!(malformed, EngineError::Invalid(ref m) if m.contains("crystalline://")),
        "{malformed:?}"
    );

    let unknown = engine
        .graph_neighborhood("crystalline://notes/ghost", 1, 100)
        .await
        .expect_err("an anchor pointing at nothing is refused");
    assert!(
        matches!(unknown, EngineError::NotFound(ref m) if m.contains("ghost")),
        "{unknown:?}"
    );
}
