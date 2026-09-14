#!/usr/bin/env bash
#
# Run the product against the ten-thousand-engram corpus and measure it.
#
# Every step runs under /usr/bin/time -l, and the runner parses the peak
# resident size and the wall clock out of that report into one CSV line per
# step. Nothing here touches the caller's config, index, state directory or
# daemon: HOME is redirected under --out, so the state directory, the socket
# and the single-instance lock all land there, and every verb that accepts
# them is given --config and --db explicitly as well.
#
#   bash evals/scale/run.sh --stage base
#   bash evals/scale/run.sh --stage daemon
#   bash evals/scale/run.sh --stage embed
#
# The stages are separate because the embedding pass takes over an hour on a
# laptop. `base` builds the index from scratch, so it must run first; `daemon`
# and `embed` both expect the index `base` left behind.
#
set -euo pipefail

BIN="target/release/crystalline"
CORPUS="evals/scale/corpus"
OUT="evals/scale/out"
STAGE="all"
DRAIN_LIMIT=1800   # seconds to wait for the daemon's embedding backlog
SAMPLE_EVERY=10    # seconds between resident-size samples

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="$2"; shift 2 ;;
    --corpus) CORPUS="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --stage) STAGE="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$STAGE" in
  all|base|embed|daemon) ;;
  *) echo "--stage must be all, base, embed or daemon" >&2; exit 2 ;;
esac

command -v "$BIN" >/dev/null 2>&1 || [ -x "$BIN" ] || {
  echo "no binary at $BIN; build one with cargo build --release" >&2
  exit 2
}
[ -d "$CORPUS" ] || {
  echo "no corpus at $CORPUS; generate one with evals/scale/generate.py" >&2
  exit 2
}

BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
CORPUS="$(cd "$CORPUS" && pwd)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

# --- isolation ---------------------------------------------------------------
#
# The state directory has no environment override of its own: it is resolved
# from the base strategy, which is rooted at HOME. Redirecting HOME is
# therefore the only way to keep this run's index.db, service.lock, socket and
# install receipt away from the caller's. The embedding model is the one thing
# deliberately shared, through CRYSTALLINE_MODELS_DIR, so the embed stage
# measures embedding rather than a 128 MB download.
REAL_HOME="$HOME"
export CRYSTALLINE_MODELS_DIR="${CRYSTALLINE_MODELS_DIR:-$REAL_HOME/.cache/crystalline/models}"
export HOME="$OUT/home"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CACHE_HOME="$HOME/.cache"
# The state directory holds the daemon's unix socket, and a unix socket path
# holds at most 103 bytes: an --out under a deep working copy overruns it and
# `serve` refuses. So the state directory alone lives at a short path, still
# outside the caller's, and STATE_DIR overrides it.
export XDG_STATE_HOME="${STATE_DIR:-/tmp/crystalline-scale}"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

CFG="$OUT/config.yaml"
DB="$OUT/index.db"
export CRYSTALLINE_CONFIG="$CFG"
LOGS="$OUT/logs"
CSV="$OUT/results.csv"
mkdir -p "$LOGS"

DOMAINS="platform observatory harbor meridian atelier"

# Ten fixed queries, every term drawn from the generated vocabulary.
QUERIES=(
  "retry queue backpressure"
  "crane move yard shuffle"
  "freezer rack chain of custody"
  "lacquer batch finishing booth"
  "flat field calibration drift"
  "rollback plan change window"
  "customs hold demurrage"
  "consent packet site binder"
  "spare holding lead time"
  "guide camera pointing model"
)

if [ ! -f "$CSV" ]; then
  echo "stage,step,exit,wall_seconds,peak_rss_mb" > "$CSV"
fi

# --- measurement -------------------------------------------------------------

# record_step <name> <exit code> <time report file>
#
# The peak resident size is reported in bytes on macOS and would be kilobytes
# on Linux, so a value under a megabyte is called out rather than quietly
# divided by the wrong constant.
record_step() {
  local name="$1" rc="$2" tim="$3"
  local bytes wall mb
  bytes="$(awk '/maximum resident set size/ {print $1}' "$tim" | tail -1)"
  wall="$(awk '/ real / && / user / {print $1}' "$tim" | tail -1)"
  if [ -z "${bytes:-}" ] || [ -z "${wall:-}" ]; then
    echo "FAILED to parse the time report for $name; see $tim" >&2
    echo "$STAGE,$name,$rc,," >> "$CSV"
    return 0
  fi
  if [ "$bytes" -lt 1000000 ]; then
    echo "SUSPECT peak resident size for $name: $bytes bytes (expected bytes, not kilobytes)" >&2
  fi
  mb="$(awk -v b="$bytes" 'BEGIN { printf "%.1f", b / 1048576 }')"
  printf '%s,%s,%s,%s,%s\n' "$STAGE" "$name" "$rc" "$wall" "$mb" >> "$CSV"
  printf '  %-34s exit %-3s %8ss %9s MB\n' "$name" "$rc" "$wall" "$mb"
}

# run_step <name> <command...>
#
# /usr/bin/time -l writes its report to stderr, and so does the command, so
# both land in the .time file and stdout alone lands in the .log file. A
# non-zero exit is a column in the CSV, never the end of the run: verify exits
# non-zero on the corpus by design, because the corpus deliberately carries
# engrams over the token budget.
run_step() {
  local name="$1"; shift
  local rc=0
  { /usr/bin/time -l "$@" > "$LOGS/$name.log"; } 2> "$LOGS/$name.time" || rc=$?
  record_step "$name" "$rc" "$LOGS/$name.time"
}

# One number out of `ctl status --json`, or an empty string when the daemon
# does not answer.
status_field() {
  "$BIN" ctl status --json 2> /dev/null \
    | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
path = sys.argv[1].split(".")
for k in path:
    d = d.get(k, {}) if isinstance(d, dict) else {}
print(d if not isinstance(d, dict) else "")' "$1" || true
}

# A search is given --config and --db everywhere except the daemon stage.
# Either flag makes the client bypass the daemon and read the index file
# itself ("Daemon: bypassed (--db/--config override)"), which while a daemon
# holds the index is a lock error, and with only --config is a silent read of
# the default index - an empty one here, answering "no results" with exit 0.
# So the routed battery passes neither and lets CRYSTALLINE_CONFIG carry the
# config, which is the shape an agent actually runs in.
search_battery() {
  local prefix="$1"; shift
  local types="$1"; shift
  local direct="${1:-direct}"
  local i=1 q t
  for q in "${QUERIES[@]}"; do
    for t in $types; do
      local name
      name="$(printf '%s-%s-%02d' "$prefix" "$t" "$i")"
      if [ "$direct" = "direct" ]; then
        run_step "$name" "$BIN" search "$q" --search-type "$t" --limit 10 \
          --config "$CFG" --db "$DB"
      else
        run_step "$name" "$BIN" search "$q" --search-type "$t" --limit 10
      fi
    done
    i=$((i + 1))
  done
}

# --- stages ------------------------------------------------------------------

stage_base() {
  echo "== base =="
  rm -f "$CFG" "$DB" "$DB"-* 2>/dev/null || true
  for d in $DOMAINS; do
    run_step "domain-add-$d" \
      "$BIN" domain add "$d" "$CORPUS/$d" --no-sync --config "$CFG" --db "$DB"
  done
  run_step "sync" "$BIN" sync --config "$CFG" --db "$DB"
  run_step "status-json" "$BIN" status --json --config "$CFG" --db "$DB"
  # verify reads files only: no index, no daemon, and its own --config means
  # a verify rule file rather than the global config, so it is not passed one.
  for d in $DOMAINS; do
    run_step "verify-$d" "$BIN" verify "$CORPUS/$d"
  done
  run_step "verify-all" "$BIN" verify \
    "$CORPUS/platform" "$CORPUS/observatory" "$CORPUS/harbor" \
    "$CORPUS/meridian" "$CORPUS/atelier"
  search_battery "search" "text hybrid semantic"
  run_step "evolve" "$BIN" evolve --limit 10 --config "$CFG" --db "$DB"
  run_step "evolve-temporal" \
    "$BIN" evolve --family temporal --limit 10 --config "$CFG" --db "$DB"
  run_step "doctor" "$BIN" doctor --config "$CFG" --db "$DB"
  run_step "reindex-full" "$BIN" reindex --full --config "$CFG" --db "$DB"
}

stage_embed() {
  echo "== embed =="
  run_step "reindex-full-embed" \
    "$BIN" reindex --full --embed --config "$CFG" --db "$DB"
  run_step "status-json-embedded" "$BIN" status --json --config "$CFG" --db "$DB"
  search_battery "embedded-search" "semantic hybrid"
}

stage_daemon() {
  echo "== daemon =="
  local rss="$OUT/daemon-rss.csv"
  local stop="$OUT/.sampler-stop"
  rm -f "$stop"
  echo "seconds,rss_mb,embedded_chunks,total_chunks,backlog" > "$rss"

  # `serve --daemon` hands off to a detached daemon and the launcher exits, so
  # the time report below covers the launch and never the daemon. The daemon's
  # own resident size is what the sampler collects, and the maximum over those
  # samples is the number this stage exists to produce.
  local tim="$LOGS/serve-daemon.time"
  { /usr/bin/time -l "$BIN" serve --daemon --http off --config "$CFG" --db "$DB" \
      > "$LOGS/serve-daemon.log"; } 2> "$tim" &
  local job=$!

  local waited=0 pid=""
  while [ "$waited" -lt 180 ]; do
    pid="$(status_field pid)"
    [ -n "$pid" ] && break
    kill -0 "$job" 2> /dev/null || break   # the launcher gave up; so do we
    sleep 2
    waited=$((waited + 2))
  done
  if [ -z "$pid" ]; then
    echo "the daemon never answered ctl status; see $tim" >&2
    kill "$job" 2> /dev/null || true
    return 1
  fi
  echo "daemon pid $pid answered after ${waited}s"

  # The sampler runs for the whole stage, so the battery's effect on the
  # daemon is sampled too and not only the embedding backlog's.
  (
    local_elapsed=0
    while [ ! -f "$stop" ]; do
      kill -0 "$pid" 2> /dev/null || {
        printf '%s,,,,daemon %s is gone\n' "$local_elapsed" "$pid" >> "$rss"
        break
      }
      size="$(ps -o rss= -p "$pid" 2> /dev/null | tr -d ' ')"
      printf '%s,%s,%s,%s,%s\n' "$local_elapsed" \
        "$(awk -v k="${size:-0}" 'BEGIN { printf "%.1f", k / 1024 }')" \
        "$(status_field embeddings.embedded_chunks)" \
        "$(status_field embeddings.total_chunks)" \
        "$(status_field activity.embedding_backlog)" >> "$rss"
      sleep "$SAMPLE_EVERY"
      local_elapsed=$((local_elapsed + SAMPLE_EVERY))
    done
  ) &
  local sampler=$!

  run_step "ctl-status" "$BIN" ctl status
  search_battery "daemon-search" "text hybrid semantic" "routed"

  # Wait for the embedding backlog the base stage left behind. A backlog that
  # outlives the limit is a recordable outcome, not a hang.
  local elapsed=0 embedded total
  while [ "$elapsed" -lt "$DRAIN_LIMIT" ]; do
    kill -0 "$pid" 2> /dev/null || { echo "the daemon died after ${elapsed}s; see $tim" >&2; break; }
    embedded="$(status_field embeddings.embedded_chunks)"
    total="$(status_field embeddings.total_chunks)"
    if [ -n "$embedded" ] && [ -n "$total" ] && [ "$embedded" = "$total" ]; then
      echo "backlog drained after about ${elapsed}s ($embedded of $total chunks)"
      break
    fi
    sleep "$SAMPLE_EVERY"
    elapsed=$((elapsed + SAMPLE_EVERY))
  done
  [ "$elapsed" -lt "$DRAIN_LIMIT" ] || echo "the backlog did not drain within ${DRAIN_LIMIT}s"

  touch "$stop"
  wait "$sampler" 2> /dev/null || true
  run_step "ctl-status-drained" "$BIN" ctl status
  run_step "ctl-shutdown" "$BIN" ctl shutdown
  wait "$job" || true
  record_step "serve-daemon-launch" 0 "$tim"

  # The daemon's peak, from the samples, as a CSV row of its own.
  local peak
  peak="$(awk -F, 'NR > 1 && $2 != "" && $2 + 0 > m { m = $2 + 0 } END { printf "%.1f", m }' "$rss")"
  printf '%s,%s,%s,%s,%s\n' "$STAGE" "daemon-peak-sampled" 0 "$elapsed" "$peak" >> "$CSV"
  printf '  %-34s exit %-3s %8ss %9s MB\n' "daemon-peak-sampled" 0 "$elapsed" "$peak"
}

case "$STAGE" in
  base) stage_base ;;
  embed) stage_embed ;;
  daemon) stage_daemon ;;
  all) stage_base; stage_daemon; stage_embed ;;
esac

echo
echo "results: $CSV"
cat "$CSV"
