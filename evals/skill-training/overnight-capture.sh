#!/usr/bin/env bash
# Unattended capture-benchmark sequence: baselines on the test split,
# then the full training run, each in a retry loop so an exhausted
# subscription usage window only delays the run instead of killing it
# (the harness fails fast and cheap while rate limited, and auto-resume
# skips all cached work on the next attempt). Gives up at the deadline.
set -uo pipefail
cd "$(dirname "$0")"

LOG=outputs/capture-overnight.log
SUMMARY=outputs/capture-overnight-summary.txt
DEADLINE_EPOCH=$(date -j -f "%Y-%m-%d %H:%M" "$(date +%Y-%m-%d) 08:30" +%s)
mkdir -p outputs

log() { echo "[$(date '+%F %T')] $*" >> "$LOG"; }

retry() {
    local name="$1"; shift
    local attempt=1
    while true; do
        log "$name: attempt $attempt"
        if "$@" >> "$LOG" 2>&1; then
            log "$name: done"
            return 0
        fi
        if [ "$(date +%s)" -ge "$DEADLINE_EPOCH" ]; then
            log "$name: giving up at deadline"
            return 1
        fi
        log "$name: failed (usage window exhausted, most likely); retrying in 15 minutes"
        sleep 900
        attempt=$((attempt + 1))
    done
}

log "overnight capture sequence starting"

retry "baseline empty skill" uv run eval_only.py --config configs/capture.yaml \
    --skill outputs/empty_skill.md --split valid_unseen \
    --out_root outputs/eval_capture_empty_test

retry "baseline shipped skill" uv run eval_only.py --config configs/capture.yaml \
    --skill outputs/seed_capture.md --split valid_unseen \
    --out_root outputs/eval_capture_seed_test

retry "full training run" uv run train.py --config configs/capture.yaml \
    --cfg-options env.out_root=outputs/capture-run1

log "writing summary"
uv run python - > "$SUMMARY" 2>&1 <<'PY'
import json
from pathlib import Path

def mean_hard(path):
    rollouts = Path(path) / "rollouts.json"
    if not rollouts.exists():
        return None
    results = json.loads(rollouts.read_text())
    return sum(r["hard"] for r in results) / max(len(results), 1)

def eval_summary(path):
    p = Path(path) / "eval_summary.json"
    return json.loads(p.read_text())["hard"] if p.exists() else None

print("capture benchmark, held-out test split (hard scores)")
print(f"  empty skill baseline:   {eval_summary('outputs/eval_capture_empty_test')}")
print(f"  shipped skill baseline: {eval_summary('outputs/eval_capture_seed_test')}")
print(f"  trained baseline check: {mean_hard('outputs/capture-run1/test_eval_baseline')}")
print(f"  best-on-val skill:      {mean_hard('outputs/capture-run1/test_eval')}")
print(f"  final skill:            {mean_hard('outputs/capture-run1/test_eval_final')}")

state = Path("outputs/capture-run1/runtime_state.json")
if state.exists():
    d = json.loads(state.read_text())
    print(f"  gate: best val {d.get('best_score')} from step {d.get('best_step')} "
          f"(last completed step {d.get('last_completed_step')})")
for record in sorted(Path("outputs/capture-run1").glob("steps/step_*/step_record.json")):
    r = json.loads(record.read_text())
    print(f"  step {r.get('step'):>2}: rollout {r.get('rollout_hard')} "
          f"candidate {r.get('candidate_gate_score')} -> {r.get('action')}")
PY
log "overnight capture sequence finished"
