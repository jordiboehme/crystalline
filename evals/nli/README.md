# NLI parity and evaluation

Two scripts for the contradiction check (V302).

- `parity.py` writes the logits fixtures `crates/index/tests/fixtures/nli-parity-<id>.json`
  from Python transformers, float32, one pair per forward pass. The Rust test
  `crates/index/tests/it/nli_model.rs` checks the candle loader against them to
  1e-4 behind `CRYSTALLINE_TEST_NLI=1`. Needs `torch`, `transformers`,
  `sentencepiece` and `protobuf`.
- `evaluate.py` reads a dump from `evals/scale/run.sh --stage nli` and the
  sidecar from `evals/scale/generate.py --contradictions`, and reports
  precision and recall per finding line, calibration, recall per flip type and
  language, noise, speed and resident size. The dump carries `similarity`, the line pair's
  cosine. `--top N` prints the product's ranking per domain. The mean table is
  unreliable since the second order is skipped below the store floor under Min.
  The probes gate is read at the product's finding line, 0.95 under min: at
  least 150 of 200 planted flips (`flips_at_0_95`) and 0 planted negatives
  (`negatives_at_0_95`); the report prints pass or FAIL. `--self-test` checks its own
  arithmetic on a tiny synthetic case.
