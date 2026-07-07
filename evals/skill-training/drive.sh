#!/usr/bin/env bash
# Unattended benchmark sequence for any env: baselines on the test
# split, then the full training run, each in a retry loop so an
# exhausted subscription usage window only delays the run instead of
# killing it (the harness fails fast and cheap while rate limited, and
# auto-resume skips all cached work on the next attempt). Gives up at
# the deadline.
#
#   bash drive.sh <config.yaml> <name> [deadline HH:MM, default 23:00]
#
# Writes outputs/<name>.log, outputs/<name>-summary.txt and run
# artifacts under outputs/<name>-run1 plus outputs/eval_<name>_*_test.
set -uo pipefail
cd "$(dirname "$0")"

CONFIG="$1"
NAME="$2"
DEADLINE="${3:-23:00}"
LOG="outputs/$NAME.log"
SUMMARY="outputs/$NAME-summary.txt"
DEADLINE_EPOCH=$(date -j -f "%Y-%m-%d %H:%M" "$(date +%Y-%m-%d) $DEADLINE" +%s)
mkdir -p outputs

SEED_FILE=$(python3 -c "
import re, sys
text = open('$CONFIG').read()
match = re.search(r'skill_init:\s*(\S+)', text)
print(match.group(1) if match else '')")
if [ -z "$SEED_FILE" ]; then
    echo "no skill_init in $CONFIG" >&2
    exit 2
fi

log() { echo "[$(date '+%F %T')] $*" >> "$LOG"; }

retry() {
    local step="$1"; shift
    local attempt=1
    while true; do
        log "$step: attempt $attempt"
        if "$@" >> "$LOG" 2>&1; then
            log "$step: done"
            return 0
        fi
        if [ "$(date +%s)" -ge "$DEADLINE_EPOCH" ]; then
            log "$step: giving up at deadline"
            return 1
        fi
        log "$step: failed (usage window exhausted, most likely); retrying in 15 minutes"
        sleep 900
        attempt=$((attempt + 1))
    done
}

log "$NAME sequence starting (config $CONFIG, deadline $DEADLINE)"

retry "baseline empty skill" uv run eval_only.py --config "$CONFIG" \
    --skill outputs/empty_skill.md --split valid_unseen \
    --out_root "outputs/eval_${NAME}_empty_test"

retry "baseline shipped skill" uv run eval_only.py --config "$CONFIG" \
    --skill "$SEED_FILE" --split valid_unseen \
    --out_root "outputs/eval_${NAME}_seed_test"

retry "full training run" uv run train.py --config "$CONFIG" \
    --cfg-options "env.out_root=outputs/${NAME}-run1"

log "writing summary"
NAME="$NAME" uv run python - > "$SUMMARY" 2>&1 <<'PY'
import json
import os
from pathlib import Path

name = os.environ["NAME"]

def mean_hard(path):
    rollouts = Path(path) / "rollouts.json"
    if not rollouts.exists():
        return None
    results = json.loads(rollouts.read_text())
    return sum(r["hard"] for r in results) / max(len(results), 1)

def eval_summary(path):
    p = Path(path) / "eval_summary.json"
    return json.loads(p.read_text())["hard"] if p.exists() else None

run = f"outputs/{name}-run1"
print(f"{name} benchmark, held-out test split (hard scores)")
print(f"  empty skill baseline:   {eval_summary(f'outputs/eval_{name}_empty_test')}")
print(f"  shipped skill baseline: {eval_summary(f'outputs/eval_{name}_seed_test')}")
print(f"  trained baseline check: {mean_hard(f'{run}/test_eval_baseline')}")
print(f"  best-on-val skill:      {mean_hard(f'{run}/test_eval')}")
print(f"  final skill:            {mean_hard(f'{run}/test_eval_final')}")

state = Path(f"{run}/runtime_state.json")
if state.exists():
    d = json.loads(state.read_text())
    print(f"  gate: best val {d.get('best_score')} from step {d.get('best_step')} "
          f"(last completed step {d.get('last_completed_step')})")
for record in sorted(Path(run).glob("steps/step_*/step_record.json")):
    r = json.loads(record.read_text())
    print(f"  step {r.get('step'):>2}: rollout {r.get('rollout_hard')} "
          f"candidate {r.get('candidate_gate_score')} -> {r.get('action')}")
PY
log "$NAME sequence finished"
