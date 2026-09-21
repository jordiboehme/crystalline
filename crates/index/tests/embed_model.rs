//! Real-model tests for the local granite provider. These download the model
//! (about 220 MB on a cold cache), so they are `#[ignore]`d and run only in the
//! dedicated cached CI job (and locally by the implementer). Run
//! single-threaded so the tests do not race on the first-download file lock:
//!
//! ```text
//! cargo test -p crystalline-index --test embed_model -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(feature = "local-embeddings")]

use std::path::Path;
use std::time::Instant;

use crystalline_core::config::EmbeddingsConfig;
use crystalline_index::{
    ChunkParams, EMBED_PAGE_SIZE, SearchMode, SearchQuery, Store, TursoStore, download_local_model,
    provider_from_config, run_embedding_pass, sync_domain_with,
};

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
}

fn local_config() -> EmbeddingsConfig {
    EmbeddingsConfig {
        provider: "local".to_string(),
        model: "granite-embedding-97m-multilingual-r2".to_string(),
        endpoint: None,
        api_key_env: None,
    }
}

#[tokio::test]
#[ignore = "downloads the real granite model"]
async fn model_download_reports_path_and_size() {
    let dl = download_local_model(&local_config()).await.unwrap();
    eprintln!(
        "model download: {} ({:.1} MB)",
        dl.path.display(),
        dl.bytes as f64 / (1024.0 * 1024.0)
    );
    assert!(dl.bytes > 1_000_000, "the weights are a real download");
    assert!(dl.path.exists());
}

#[tokio::test]
#[ignore = "downloads the real granite model"]
async fn semantic_query_without_term_overlap_ranks_related_engram_top_three() {
    let cfg = local_config();
    let provider = provider_from_config(&cfg).await.unwrap();
    assert_eq!(
        provider.dims(),
        384,
        "granite-embedding-97m-multilingual-r2 is 384 dimensional"
    );

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Target: about authentication, but avoiding the query's surface words
    // (no "authentication", "login", "flow" or "user").
    write(
        root,
        "signing-in.md",
        &engram(
            "Signing in",
            "signing-in",
            "When someone proves who they are, the server hands back a signed session \
             credential that the browser keeps and presents on each later visit so they \
             stay recognized without typing their password again.",
        ),
    );
    // Decoy: shares the query's surface words but is about irrigation.
    write(
        root,
        "canal.md",
        &engram(
            "Canal control",
            "canal",
            "The authentication flow in the irrigation user manual explains how login \
             valves and flow meters regulate water across the canal network.",
        ),
    );
    // Filler engrams so top-3 is a real ranking.
    write(
        root,
        "pasta.md",
        &engram(
            "Pasta",
            "pasta",
            "Boil the pasta in salted water then toss it with olive oil, garlic and basil.",
        ),
    );
    write(
        root,
        "weather.md",
        &engram(
            "Weather",
            "weather",
            "A cold front will bring rain and gusty winds to the coast overnight.",
        ),
    );
    write(
        root,
        "cycling.md",
        &engram(
            "Cycling",
            "cycling",
            "Keep a steady cadence on the climb and shift down early before the gradient steepens.",
        ),
    );

    let store = TursoStore::open_in_memory().await.unwrap();
    let params = ChunkParams::for_model(provider.model_id());
    sync_domain_with(&store, "d", root, &params).await.unwrap();

    let started = Instant::now();
    let report = run_embedding_pass(&store, provider.as_ref(), |done, total| {
        eprintln!("  embedded {done}/{total}");
    })
    .await
    .unwrap();
    let secs = started.elapsed().as_secs_f64();
    eprintln!(
        "embedded {} chunks in {:.2}s ({:.1} chunks/s)",
        report.chunks,
        secs,
        report.chunks as f64 / secs.max(1e-6)
    );

    // A query with no term overlap with the target, embedded through
    // embed_queries so the model's own query convention applies.
    let qtext = "how do users authenticate and log in";
    let qvec = provider
        .embed_queries(&[qtext.to_string()])
        .await
        .unwrap()
        .remove(0);
    let page = store
        .search(&SearchQuery {
            text: Some(qtext.to_string()),
            mode: SearchMode::Semantic,
            query_embedding: Some(qvec),
            active_model: Some(provider.model_id().to_string()),
            min_similarity: Some(0.0),
            limit: 10,
            page: 1,
            ..SearchQuery::default()
        })
        .await
        .unwrap();

    let top3: Vec<&str> = page
        .items
        .iter()
        .take(3)
        .map(|h| h.permalink.as_str())
        .collect();
    eprintln!("top 3: {top3:?}");
    assert!(
        top3.contains(&"signing-in"),
        "the semantically related engram is in the top 3 despite no term overlap: {top3:?}"
    );

    // Re-syncing the unchanged corpus embeds nothing.
    sync_domain_with(&store, "d", root, &params).await.unwrap();
    let pending = store
        .chunks_needing_embedding(provider.model_id(), None, EMBED_PAGE_SIZE, None)
        .await
        .unwrap();
    assert!(pending.is_empty(), "a warm resync re-embeds nothing");
    eprintln!("warm resync: 0 re-embeds");
}

/// The point of the model swap: one fact said in two languages has to sit
/// closer together than either does to an unrelated fact in the same language.
#[tokio::test]
#[ignore = "downloads the real granite model"]
async fn a_german_statement_is_closer_to_its_english_twin_than_to_an_unrelated_one() {
    let provider = provider_from_config(&local_config()).await.unwrap();
    let vectors = provider
        .embed(&[
            "Der Build nutzt Node 18".to_string(),
            "The build uses Node 18".to_string(),
            "Deployments run on Fridays".to_string(),
        ])
        .await
        .unwrap();
    let twin = cosine(&vectors[0], &vectors[1]);
    let unrelated = cosine(&vectors[0], &vectors[2]);
    eprintln!("cosine: de/en twin {twin:.4}, unrelated {unrelated:.4}");
    assert!(
        twin > unrelated,
        "the German sentence is closer to its English twin ({twin:.4}) than to an unrelated English one ({unrelated:.4})"
    );
}

/// Gate 1 of the granite ruling (spec section 10): the vendored, patched
/// ModernBERT module reproduces the vendor's own ONNX export. A wrong
/// activation lands at cosine 0.78 to 0.88 and never trips a shape error, so
/// this is the only thing that proves the vectors are right. One text per
/// call first (no padding can differ), then the five as one padded batch,
/// which is the pad-id and mask check.
#[tokio::test]
#[ignore = "downloads the real granite model"]
async fn the_vendored_modernbert_reproduces_the_onnx_reference_vectors() {
    #[derive(serde::Deserialize)]
    struct Fixture {
        tolerance: Tolerance,
        items: Vec<Item>,
    }
    #[derive(serde::Deserialize)]
    struct Tolerance {
        min_cosine: f32,
        max_abs_diff: f32,
    }
    #[derive(serde::Deserialize)]
    struct Item {
        label: String,
        text: String,
        vector: Vec<f32>,
    }
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/granite-parity.json")).unwrap();
    let provider = provider_from_config(&local_config()).await.unwrap();

    let mut single = Vec::new();
    for item in &fixture.items {
        let v = provider
            .embed(std::slice::from_ref(&item.text))
            .await
            .unwrap()
            .remove(0);
        let cos = cosine(&v, &item.vector);
        let diff = v
            .iter()
            .zip(&item.vector)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        eprintln!("{}: cosine {cos:.9}, max abs diff {diff:.2e}", item.label);
        assert!(
            cos >= fixture.tolerance.min_cosine,
            "{}: cosine {cos}",
            item.label
        );
        assert!(
            diff <= fixture.tolerance.max_abs_diff,
            "{}: max abs diff {diff}",
            item.label
        );
        single.push(v);
    }

    let texts: Vec<String> = fixture.items.iter().map(|i| i.text.clone()).collect();
    let batched = provider.embed(&texts).await.unwrap();
    for ((item, one), many) in fixture.items.iter().zip(&single).zip(&batched) {
        let diff = one
            .iter()
            .zip(many)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        eprintln!(
            "{}: padded batch vs single call, max abs diff {diff:.2e}",
            item.label
        );
        assert!(
            diff <= fixture.tolerance.max_abs_diff,
            "{}: the padded batch differs from the single call by {diff}",
            item.label
        );
    }
}

/// Unit vectors, so the dot product is the cosine.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
