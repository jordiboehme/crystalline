//! `V301` end to end: real writes, real embeddings from the topic provider,
//! the real sweep. A paraphrase pair (the same words in a different order)
//! is invisible to the lexical `V201` and visible to `V301`; an exact copy is
//! `V201`'s and never doubled as a twin.

mod support;

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::params::{EditParams, EvolveParams, WriteParams};
use crystalline_service::{Engine, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

// Three texts on one topic. `A` and `PARAPHRASE` share the topic marker words
// (retry, queue, backoff, dead-letter, ttl - what `TopicEmbedder` reads) and
// share NO three-word run and few character bigrams, so `V201`'s MinHash
// blocking and Dice verification both miss them while their lead vectors are
// identical. `THIRD` is a further rewording. A verbatim copy of `A` is what
// `V201` is for.
const A: &str = "The retry queue doubles its backoff on every failure and a dead-letter ttl bounds how long a retry waits.\nRaising the ttl fixed the stuck retries last time.\nA retry storm needs a wider backoff on the queue.";
const PARAPHRASE: &str = "Each failed attempt makes the queue back off twice as long, while the dead-letter ttl is the ceiling on any single retry.\nWhen retries stalled before, a bigger ttl cleared them.\nBursty retry traffic wants more generous backoff across the queue.";
const THIRD: &str = "Retries back off with doubling delays; the dead-letter ttl caps the wait per retry.\nA longer ttl once unstuck the whole queue.\nUnder a burst of retries the queue needs a slower backoff curve.";

/// One virtual domain on an engine with, or without, the topic provider
/// installed. Virtual so nothing touches disk and every write goes straight to
/// the index.
async fn engine(with_provider: bool) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let provider: Option<Arc<dyn crystalline_index::EmbeddingProvider>> =
        with_provider.then(|| Arc::new(support::TopicEmbedder) as Arc<_>);
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        provider,
        Some(config_path),
    ));
    (tmp, engine)
}

fn write(title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: vec!["t".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
    }
}

async fn sweep(engine: &Engine) -> Value {
    engine
        .evolve_engrams(
            &EvolveParams {
                domains: vec!["notes".to_string()],
                families: vec!["redundancy".to_string()],
                rules: Vec::new(),
                min_priority: None,
                limit: Some(50),
                page: None,
                today: Some("2026-09-07".to_string()),
                include_acknowledged: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

/// Every queue row as `(rule, permalink)`. The response calls the rows `queue`,
/// which is what a renderer reads.
fn rules_of(value: &Value) -> Vec<(String, String)> {
    value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["rule"].as_str().unwrap().to_string(),
                f["permalink"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The evidence of every `V301` row, in queue order.
fn twin_evidence(value: &Value) -> Vec<String> {
    value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "V301")
        .map(|f| f["evidence"].as_str().unwrap().to_string())
        .collect()
}

/// The embeddings have to be under the engine's active model, not the
/// provider's own id, or the sweep's fetch finds nothing and every twin
/// assertion below fails for a reason that has nothing to do with the rule.
async fn assert_embedded(engine: &Engine) {
    let store = engine.store();
    let store = store.lock().await;
    let coverage = store.embedding_coverage().await.unwrap();
    assert!(
        coverage.embedded_for(engine.model_id()) > 0,
        "nothing embedded under the active model '{}': {coverage:?}",
        engine.model_id()
    );
}

#[tokio::test]
async fn a_paraphrase_is_a_twin_and_a_copy_is_a_duplicate() {
    let (_tmp, engine) = engine(true).await;
    engine
        .write_engram(&write("Retry queue gotcha", A))
        .await
        .unwrap();
    engine
        .write_engram(&write("Backoff lesson", PARAPHRASE))
        .await
        .unwrap();
    engine
        .write_engram(&write("Retry queue copy", A))
        .await
        .unwrap();
    engine
        .write_engram(&write(
            "Docking clamps",
            "Clamp three reads locked before it seats in the aft bay.\nWait for the green tone before cutting thrust on docking.\nThe clamps misread below eight degrees.",
        ))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_embedded(&engine).await;

    let value = sweep(&engine).await;
    let rules = rules_of(&value);
    assert!(
        rules.iter().any(|(r, _)| r == "V201"),
        "the copy is lexical: {rules:?}"
    );
    let twins: Vec<&(String, String)> = rules.iter().filter(|(r, _)| r == "V301").collect();
    assert_eq!(
        twins.len(),
        2,
        "the paraphrase twins each copy, the copies pair is V201's: {rules:?}"
    );
    let evidence = twin_evidence(&value);
    assert!(
        evidence
            .iter()
            .all(|e| e.contains("backoff-lesson") || e.contains("retry-queue")),
        "{evidence:?}"
    );
    assert!(!evidence.iter().any(|e| e.contains("docking-clamps")));
}

#[tokio::test]
async fn without_a_provider_the_rule_is_silent() {
    let (_tmp, engine) = engine(false).await;
    engine
        .write_engram(&write("Retry queue gotcha", A))
        .await
        .unwrap();
    engine
        .write_engram(&write("Backoff lesson", PARAPHRASE))
        .await
        .unwrap();
    let value = sweep(&engine).await;
    assert!(!rules_of(&value).iter().any(|(r, _)| r == "V301"));
}

/// The `evolve_ack` edit an agent sends for one engram.
fn ack(permalink: &str) -> EditParams {
    EditParams {
        identifier: permalink.to_string(),
        domain: "notes".to_string(),
        operation: "set_frontmatter".to_string(),
        key: Some("evolve_ack".to_string()),
        value: Some("V301 distinct, linked".to_string()),
        content: None,
        section: None,
        find_text: None,
        expected_checksum: None,
        expected_replacements: None,
        include_subsections: false,
        ack_scope: None,
    }
}

/// Three engrams on one topic: every pair is a twin, so the middle one by
/// address leads two findings and is the hub the pair scoping has to survive.
async fn three_twins(engine: &Engine) {
    engine
        .write_engram(&write("Retry queue gotcha", A))
        .await
        .unwrap();
    engine
        .write_engram(&write("Backoff lesson", PARAPHRASE))
        .await
        .unwrap();
    engine
        .write_engram(&write("Retry storm note", THIRD))
        .await
        .unwrap();
    engine.embed_pending().await.unwrap();
    assert_embedded(engine).await;
}

#[tokio::test]
async fn acknowledging_one_pair_leaves_the_other_standing() {
    let (_tmp, engine) = engine(true).await;
    three_twins(&engine).await;
    // One sweep serves both the count and the row to acknowledge: the ack is
    // stamped on the engram a twin finding fires on, and the scope it records
    // is that finding's pair, resolved by the same `sweep_domain` this task
    // wires - which is the point of this test.
    let value = sweep(&engine).await;
    let twins_before = rules_of(&value).iter().filter(|(r, _)| r == "V301").count();
    assert!(twins_before >= 2, "{value}");
    let lead = value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["rule"] == "V301"
                && f["evidence"]
                    .as_str()
                    .unwrap()
                    .contains("retry-queue-gotcha")
        })
        .expect("a twin finding naming retry-queue-gotcha");
    let permalink = lead["permalink"].as_str().unwrap().to_string();
    engine.edit_engram(&ack(&permalink)).await.unwrap();
    // The edit re-chunks the engram it stamps, so its lead vector is gone
    // until the next embed pass. In the daemon the worker does this; here the
    // test does, or the pair would vanish for the wrong reason.
    engine.embed_pending().await.unwrap();
    let after = sweep(&engine).await;
    assert_eq!(
        after["acknowledged"]["by_family"]["redundancy"], 1,
        "{after}"
    );
    assert_eq!(
        rules_of(&after).iter().filter(|(r, _)| r == "V301").count(),
        twins_before - 1
    );
    // The acknowledged engram leads a second pair, and that row is a plain
    // finding: staleness is judged per pair, so an entry given for one pair
    // never lends its note to another.
    assert!(
        after["queue"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["rule"] == "V301")
            .all(|f| f["ack_stale"] != true && f["ack_note"].is_null()),
        "{after}"
    );
}

/// An engram that twins two others carries two `V301` findings, and an
/// acknowledgment is given for a pair rather than for a rule: the second one
/// is stored beside the first instead of replacing it, each silences its own
/// finding, and neither leaves the other marked stale.
#[tokio::test]
async fn two_pairs_on_one_hub_are_acknowledged_side_by_side() {
    let (_tmp, engine) = engine(true).await;
    three_twins(&engine).await;
    let before = sweep(&engine).await;
    let hub = before["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "V301")
        .map(|f| f["permalink"].as_str().unwrap().to_string())
        .fold(std::collections::HashMap::new(), |mut counts, p| {
            *counts.entry(p).or_insert(0usize) += 1;
            counts
        })
        .into_iter()
        .find(|(_, n)| *n == 2)
        .map(|(p, _)| p)
        .expect("one engram leads two twin findings");

    // Two acknowledgments, one act each. The scope is the server's to resolve,
    // so the caller sends the same value twice and the two entries differ by
    // the pair they were given for.
    let first = engine.edit_engram(&ack(&hub)).await.unwrap();
    engine.embed_pending().await.unwrap();
    let second = engine.edit_engram(&ack(&hub)).await.unwrap();
    engine.embed_pending().await.unwrap();
    assert_ne!(
        first["evolve_ack"]["scope"], second["evolve_ack"]["scope"],
        "the second acknowledgment is a different pair: {first} / {second}"
    );

    let after = sweep(&engine).await;
    assert_eq!(
        after["acknowledged"]["by_family"]["redundancy"], 2,
        "{after}"
    );
    let rows: Vec<&Value> = after["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "V301")
        .collect();
    assert_eq!(rows.len(), 1, "the third pair still stands: {after}");
    assert_ne!(
        rows[0]["permalink"].as_str().unwrap(),
        hub,
        "the standing finding is the pair the hub does not lead"
    );
    assert!(
        rows[0]["ack_stale"] != true && rows[0]["ack_note"].is_null(),
        "a pair nobody acknowledged is not stale: {}",
        rows[0]
    );
}

/// Every `V301` row on `permalink`, as `(scope, evidence)` in queue order. The
/// scope is what names the pair on the wire: the row a person clicks and the
/// acknowledgment they ask for have to be the same pair, and this is the only
/// field that says which.
fn pairs_on(value: &Value, permalink: &str) -> Vec<(String, String)> {
    value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "V301" && f["permalink"] == permalink)
        .map(|f| {
            (
                f["scope"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a twin row carries its pair: {f}"))
                    .to_string(),
                f["evidence"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The engram that leads two twin findings.
fn hub_of(value: &Value) -> String {
    value["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "V301")
        .map(|f| f["permalink"].as_str().unwrap().to_string())
        .fold(std::collections::HashMap::new(), |mut counts, p| {
            *counts.entry(p).or_insert(0usize) += 1;
            counts
        })
        .into_iter()
        .find(|(_, n)| *n == 2)
        .map(|(p, _)| p)
        .expect("one engram leads two twin findings")
}

/// The pair a person clicked is the pair that gets acknowledged, not whichever
/// one the server would have picked. A hub leads two twin findings; naming the
/// **second** row's pair silences that row and leaves the first standing.
#[tokio::test]
async fn an_acknowledgment_is_given_for_the_pair_the_caller_names() {
    let (_tmp, engine) = engine(true).await;
    three_twins(&engine).await;
    let before = sweep(&engine).await;
    let hub = hub_of(&before);
    let rows = pairs_on(&before, &hub);
    assert_eq!(rows.len(), 2, "{before}");
    let (second_pair, second_evidence) = rows[1].clone();

    let entry = engine
        .acknowledge_finding_as(
            "notes",
            &hub,
            "V301",
            Some("distinct, linked"),
            Some(&second_pair),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        entry["scope"], second_pair,
        "the entry names the pair the caller did: {entry}"
    );

    engine.embed_pending().await.unwrap();
    let after = sweep(&engine).await;
    let standing = pairs_on(&after, &hub);
    assert_eq!(standing.len(), 1, "one of the hub's pairs is silenced");
    assert_ne!(
        standing[0].1, second_evidence,
        "the row that stands is the one nobody named"
    );
    assert_eq!(standing[0].0, rows[0].0, "and it is the first pair");
}

/// A pair that is not firing cannot be acknowledged: the caller is naming
/// evidence the sweep does not see, which is a queue they read too long ago.
#[tokio::test]
async fn a_pair_the_sweep_does_not_see_is_refused() {
    let (_tmp, engine) = engine(true).await;
    three_twins(&engine).await;
    let before = sweep(&engine).await;
    let hub = hub_of(&before);

    let refused = engine
        .acknowledge_finding_as(
            "notes",
            &hub,
            "V301",
            None,
            Some("crystalline://notes/nobody, crystalline://notes/nothing"),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("V301"),
        "the refusal names the rule: {refused}"
    );
}

/// A withdrawal names a pair too, so taking one acknowledgment back leaves the
/// hub's other pair silenced.
#[tokio::test]
async fn withdrawing_one_pair_leaves_the_other_acknowledged() {
    let (_tmp, engine) = engine(true).await;
    three_twins(&engine).await;
    let before = sweep(&engine).await;
    let hub = hub_of(&before);
    let rows = pairs_on(&before, &hub);
    for (pair, _) in &rows {
        engine
            .acknowledge_finding_as("notes", &hub, "V301", Some("linked"), Some(pair), None)
            .await
            .unwrap();
        engine.embed_pending().await.unwrap();
    }

    let removed = engine
        .unacknowledge_finding_as("notes", &hub, "V301", Some(&rows[0].0), None)
        .await
        .unwrap();
    assert!(removed);
    engine.embed_pending().await.unwrap();

    let after = sweep(&engine).await;
    let standing = pairs_on(&after, &hub);
    assert_eq!(
        standing.len(),
        1,
        "the withdrawn pair is back and the other stays silenced: {after}"
    );
    assert_eq!(standing[0].0, rows[0].0);
}
