//! The real NLI checkpoints, behind `CRYSTALLINE_TEST_NLI=1` (369 to 558 MB
//! each on a cold cache): logits parity against Python transformers for every
//! profile, run as one mixed-length padded batch through the real path, then
//! the flipped-fact checks for the two multilingual profiles. The measurement
//! does not start until the parity test passes for all three. Skipped with a
//! note otherwise, like the postgres leg.
//!
//! Point `CRYSTALLINE_MODELS_DIR` at a directory of its own: a daemon sharing
//! the cache prunes checkpoints its setting does not name.
//!
//! Run it on the release profile: an unoptimized candle takes many minutes
//! over the 270-token parity batch, the release build about twenty seconds.
//!
//! ```text
//! CRYSTALLINE_TEST_NLI=1 cargo nextest run --cargo-profile release -p crystalline-index --test nli_model --no-capture -j 1
//! ```

#![cfg(feature = "local-embeddings")]

use crystalline_index::nli::local::LocalNli;
use crystalline_index::nli::{ContradictionScorer, NLI_MODELS, NliProfile, nli_model};
use crystalline_index::sweep::{CONTRADICTION_STORE_FLOOR, ORDER_AGGREGATION};

fn enabled() -> bool {
    if std::env::var("CRYSTALLINE_TEST_NLI").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("note: skipping the real NLI model tests (CRYSTALLINE_TEST_NLI is not 1)");
    false
}

#[tokio::test]
async fn nli_parity_holds_for_every_profile_as_one_padded_batch() {
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
            "parity {}: {} pairs, max abs logit diff {max_diff:e} (tolerance {tolerance})",
            model.id,
            pairs.len()
        );
    }
    assert!(misses.is_empty(), "parity misses:\n{}", misses.join("\n"));
}

/// Every profile is scored and printed first, the English one included for
/// the record, so a miss still leaves all the numbers in the log; only the
/// two multilingual profiles are held to the German pair.
#[tokio::test]
async fn the_multilingual_profiles_separate_a_flipped_fact_from_an_unrelated_one() {
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
    for (profile, model, flipped, unrelated, flipped_de) in scored {
        if profile == NliProfile::EnglishOnly {
            continue;
        }
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
        assert!(
            flipped_de >= model.threshold,
            "{}: German flipped fact scored {flipped_de}",
            model.id
        );
    }
}
