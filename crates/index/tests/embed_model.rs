//! Real-model tests for the local bge provider. These download the model, so
//! they are `#[ignore]`d and run only in the dedicated cached CI job (and
//! locally by the implementer). Run single-threaded so the two tests do not race
//! on the first-download file lock:
//!
//! ```text
//! cargo test -p crystalline-index --test embed_model -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(feature = "local-embeddings")]

use std::path::Path;
use std::time::Instant;

use crystalline_core::config::EmbeddingsConfig;
use crystalline_index::{
    ChunkParams, SearchMode, SearchQuery, Store, TursoStore, download_local_model,
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
        model: "bge-small-en-v1.5".to_string(),
        endpoint: None,
        api_key_env: None,
    }
}

#[tokio::test]
#[ignore = "downloads the real bge model"]
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
#[ignore = "downloads the real bge model"]
async fn semantic_query_without_term_overlap_ranks_related_engram_top_three() {
    let cfg = local_config();
    let provider = provider_from_config(&cfg).await.unwrap();
    assert_eq!(provider.dims(), 384, "bge-small is 384 dimensional");

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

    // A query with no term overlap with the target, embedded with the bge query
    // instruction prefix via embed_queries.
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
        .chunks_needing_embedding(provider.model_id())
        .await
        .unwrap();
    assert!(pending.is_empty(), "a warm resync re-embeds nothing");
    eprintln!("warm resync: 0 re-embeds");
}
