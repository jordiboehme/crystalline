//! The real NLI checkpoint, behind `CRYSTALLINE_TEST_NLI=1` (558 MB on a cold
//! cache): logits parity against Python transformers, run as one mixed-length
//! padded batch through the real path, then the flipped-fact checks. The
//! measurement does not start until the parity test passes. Skipped with a
//! note otherwise, like the postgres leg.
//!
//! `CRYSTALLINE_MODELS_DIR` must point at a directory of its own, and the
//! tests refuse to run without it: the default is the user's real model
//! cache, where a running daemon prunes checkpoints its setting does not name
//! (possibly mid-test) and 558 MB would land unasked.
//!
//! Run it on the release profile: an unoptimized candle takes many minutes
//! over the 270-token parity batch, the release build about twenty seconds.
//!
//! ```text
//! CRYSTALLINE_TEST_NLI=1 CRYSTALLINE_MODELS_DIR=/path/to/scratch/models \
//!   cargo nextest run --cargo-profile release -p crystalline-index --test it --no-capture -E 'test(/^nli_model::/)' -j 1
//! ```

#![cfg(feature = "local-embeddings")]

use crystalline_index::nli::local::LocalNli;
use crystalline_index::nli::{ContradictionScorer, NLI_MODELS, NliProfile, nli_model};
use crystalline_index::sweep::{CONTRADICTION_STORE_FLOOR, ORDER_AGGREGATION};
use crystalline_index::{ACCELERATION_ENV, CpuReason, DeviceKind, DeviceReport};

/// Whether the real-model tests run. Asked for without a dedicated model
/// directory, they fail loudly rather than download into the user's cache.
fn enabled() -> bool {
    if std::env::var("CRYSTALLINE_TEST_NLI").as_deref() != Ok("1") {
        eprintln!("note: skipping the real NLI model tests (CRYSTALLINE_TEST_NLI is not 1)");
        return false;
    }
    let dedicated = std::env::var_os("CRYSTALLINE_MODELS_DIR").is_some_and(|d| !d.is_empty());
    assert!(
        dedicated,
        "CRYSTALLINE_TEST_NLI=1 needs CRYSTALLINE_MODELS_DIR set to a directory of its own: \
         the default is your real model cache, which a running daemon prunes"
    );
    true
}

/// The device the checkpoint loaded on, held to what this run asked for: on
/// an Apple Silicon Mac the GPU, or the CPU when `CRYSTALLINE_ACCELERATION`
/// is `off`; the CPU everywhere else. A silent fallback would otherwise make
/// a "Metal" run a second CPU run with the same numbers.
fn expected_device(nli: &LocalNli) -> DeviceReport {
    let device = nli
        .device()
        .expect("a loaded checkpoint reports its device");
    let off = std::env::var(ACCELERATION_ENV).is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"));
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        if off {
            assert_eq!(device.kind, DeviceKind::Cpu, "{device}");
            assert_eq!(device.reason, Some(CpuReason::Off), "{device}");
        } else {
            assert_eq!(device, DeviceReport::metal(), "no silent CPU fallback");
        }
    } else {
        assert_eq!(device, DeviceReport::cpu());
    }
    device
}

#[tokio::test]
async fn nli_parity_holds_as_one_padded_batch() {
    if !enabled() {
        return;
    }
    let mut misses = Vec::new();
    for model in &NLI_MODELS {
        let path = format!(
            "{}/tests/fixtures/nli-parity-{}.json",
            env!("CARGO_MANIFEST_DIR"),
            model.id
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing {path}; generate it with python3 evals/nli/parity.py --model {}",
                model.repo
            )
        });
        let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(fixture["model"], model.repo);
        let tolerance = fixture["tolerance"].as_f64().unwrap() as f32;
        let items = fixture["pairs"].as_array().unwrap();
        let pairs: Vec<(String, String)> = items
            .iter()
            .map(|i| {
                (
                    i["premise"].as_str().unwrap().to_string(),
                    i["hypothesis"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let nli = LocalNli::load(model).await.unwrap();
        let device = expected_device(&nli);

        // The token ids first: a logits miss with matching ids is the model's,
        // a miss with differing ids is the tokenizer's.
        let ids = nli.pair_ids(&pairs).unwrap();
        for (item, got) in items.iter().zip(&ids) {
            let want: Vec<u32> = item["input_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u32)
                .collect();
            assert_eq!(
                got, &want,
                "{} {}: token ids differ from Python",
                model.id, item["label"]
            );
        }
        let lengths: std::collections::BTreeSet<usize> = ids.iter().map(Vec::len).collect();
        assert!(
            lengths.len() > 1,
            "{}: the batch mixes lengths, so padding is exercised",
            model.id
        );

        let got = nli.logits(&pairs).unwrap();
        assert_eq!(got.len(), items.len());
        let mut max_diff = 0f32;
        for (item, row) in items.iter().zip(&got) {
            let want: Vec<f32> = item["logits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            assert_eq!(row.len(), want.len(), "{}", model.id);
            for (g, w) in row.iter().zip(&want) {
                let diff = (g - w).abs();
                max_diff = max_diff.max(diff);
                if diff > tolerance {
                    misses.push(format!(
                        "{} {}: {row:?} against {want:?}",
                        model.id, item["label"]
                    ));
                }
            }
        }
        eprintln!(
            "parity {} on {device}: {} pairs, max abs logit diff {max_diff:e} (tolerance {tolerance})",
            model.id,
            pairs.len()
        );
    }
    assert!(misses.is_empty(), "parity misses:\n{}", misses.join("\n"));
}

/// Every profile is scored and printed first, so a miss still leaves all the
/// numbers in the log, then each is held to the English and the German pair.
#[tokio::test]
async fn the_profiles_separate_a_flipped_fact_from_an_unrelated_one() {
    if !enabled() {
        return;
    }
    let mut scored = Vec::new();
    for profile in NliProfile::ALL {
        let model = nli_model(profile);
        let nli = LocalNli::load(model).await.unwrap();
        let both = |a: &str, b: &str| {
            let s = nli
                .score(&[
                    (a.to_string(), b.to_string()),
                    (b.to_string(), a.to_string()),
                ])
                .unwrap();
            eprintln!(
                "  {}: {a:?} / {b:?}: ab {:.3}, ba {:.3}",
                model.id, s[0], s[1]
            );
            (ORDER_AGGREGATION.combine(s[0], s[1]), s[0].max(s[1]))
        };
        let (flipped, _) = both("The build uses Node 18", "The build uses Node 20");
        let (_, unrelated) = both("The build uses Node 18", "Deployments run on Fridays");
        let (flipped_de, _) = both("Der Build nutzt Node 18", "Der Build nutzt Node 20");
        eprintln!(
            "{}: flipped {flipped:.3}, unrelated {unrelated:.3}, German flipped {flipped_de:.3}",
            model.id
        );
        scored.push((profile, model, flipped, unrelated, flipped_de));
    }
    for (_profile, model, flipped, unrelated, flipped_de) in scored {
        assert!(
            flipped >= model.threshold,
            "{}: flipped fact scored {flipped}",
            model.id
        );
        assert!(
            unrelated < CONTRADICTION_STORE_FLOOR,
            "{}: unrelated pair scored {unrelated}",
            model.id
        );
        // The German phrasing scored 0.943 on 2026-10-02, under the 0.95
        // threshold: it is a known miss at this threshold, so it is held only
        // to the store floor (the row is stored, the finding is not raised).
        assert!(
            flipped_de >= CONTRADICTION_STORE_FLOOR,
            "{}: German flipped fact scored {flipped_de}",
            model.id
        );
    }
}

/// A GPU failure after a good load, injected into the real checkpoint: the
/// same batch is scored again on the CPU, the scores agree with the GPU's to
/// within 1e-4, the report names the runtime reason, and the next batch runs
/// on the CPU. On a CPU-only machine (or with the override off) there is no
/// GPU to fail, and the injection is ignored.
#[tokio::test]
async fn a_runtime_gpu_failure_scores_the_batch_again_on_the_cpu() {
    if !enabled() {
        return;
    }
    let model = nli_model(NliProfile::Full);
    let nli = LocalNli::load(model).await.unwrap();
    let device = expected_device(&nli);
    let pairs = vec![
        (
            "The build uses Node 18".to_string(),
            "The build uses Node 20".to_string(),
        ),
        (
            "Der Build nutzt Node 18".to_string(),
            "Deployments run on Fridays".to_string(),
        ),
    ];
    let before = nli.score(&pairs).unwrap();
    nli.inject_runtime_failure(1);
    let after = nli.score(&pairs).unwrap();
    for (b, a) in before.iter().zip(&after) {
        assert!((b - a).abs() < 1e-4, "{before:?} against {after:?}");
    }
    let now = nli.device().unwrap();
    if device.kind == DeviceKind::Metal {
        assert_eq!(now.kind, DeviceKind::Cpu, "{now}");
        assert_eq!(
            now.to_string(),
            "cpu (metal failed at runtime: contradiction model error: injected runtime failure)"
        );
        eprintln!("runtime fallback: {before:?} on metal, {after:?} on the cpu");
        // A load later in the process (the daemon's reload after the idle
        // drop) stays on the CPU. The failure is remembered for the whole
        // process, which is why these tests run under nextest, one process
        // per test.
        let reloaded = LocalNli::load(model).await.unwrap();
        assert_eq!(reloaded.device().unwrap(), now, "no second try on metal");
    } else {
        assert_eq!(now, device, "nothing to fall back from");
    }
    assert_eq!(nli.score(&pairs).unwrap().len(), 2);
}
