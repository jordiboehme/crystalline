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
#   bash evals/scale/run.sh --stage embed
#   bash evals/scale/run.sh --stage daemon
#   bash evals/scale/run.sh --stage nli --nli-profile full [--nli-lift no]
#
# The stages are separate because the embedding pass takes over an hour on a
# laptop, and they run in that order: `base` builds the index from scratch and
# leaves every chunk waiting to be embedded, `embed` measures that bulk pass and
# its peak, `daemon` then serves the already-embedded index and samples what the
# battery costs it. The order is load bearing rather than a habit - `reindex
# --full` keeps the embedding of every chunk whose text is unchanged, and the
# daemon drains the backlog on its own, so an embed stage run after the daemon
# stage embeds nothing at all. It refuses rather than reporting that nothing
# took no time.
#
# `nli` sets `evolve.contradictions` to one profile, starts the daemon, waits
# until nothing is pending, samples the resident size until the model is
# dropped, and dumps every stored row. By default (`--nli-lift yes`) it lowers
# the related line to 0.70 and lifts the pair cap, so higher lines are
# evaluated off the stored cosine without rescoring: run that on a probes
# corpus (`generate.py --domains 1 --engrams-per-domain 200 --contradictions
# 400`), because on the ten-thousand-engram corpus a lifted cap means weeks of
# CPU (its lead cosines are degenerate, see `TWIN_THRESHOLD`). `--nli-lift no`
# keeps the product's own caps: that is the drain time and resident size of
# the full corpus. A pre-flight projects the drain from the first pass and
# aborts when it would exceed `DRAIN_LIMIT`. Each profile runs with its own
# model cache, because a daemon prunes every NLI checkpoint but its own at
# start.
#
set -euo pipefail

BIN="target/release/crystalline"
CORPUS="evals/scale/corpus"
OUT="evals/scale/out"
STAGE="all"
DRAIN_LIMIT="${DRAIN_LIMIT:-1800}"   # seconds to wait for the daemon's embedding backlog
SAMPLE_EVERY=10    # seconds between resident-size samples
NLI_PROFILE="full"
NLI_LIFT="yes"
NLI_UNLOAD_WAIT="${NLI_UNLOAD_WAIT:-960}"   # seconds to keep sampling after the drain, for the idle drop
NLI_SEED_MODELS="${NLI_SEED_MODELS:-}"   # a directory already holding this profile's checkpoint, hf-hub cache shape

while [ $# -gt 0 ]; do
  case "$1" in
    --bin) BIN="$2"; shift 2 ;;
    --corpus) CORPUS="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --stage) STAGE="$2"; shift 2 ;;
    --nli-profile) NLI_PROFILE="$2"; shift 2 ;;
    --nli-lift) NLI_LIFT="$2"; shift 2 ;;
    # The header block above, which ends at the line before `set -euo
    # pipefail`, so --help never truncates mid-sentence when it is edited.
    -h|--help) sed -n '2,/^set -euo pipefail$/p' "$0" | sed '$d'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$STAGE" in
  all|base|embed|daemon|nli) ;;
  *) echo "--stage must be all, base, embed, daemon or nli" >&2; exit 2 ;;
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
# tracing-subscriber writes ANSI color codes into daemon.log unless told not
# to, and both log parsers below (the nli pre-flight and evaluate.py's
# speed()) have to read those lines back out.
export NO_COLOR=1
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

# Every corpus directory with a MANIFEST.md: the five generated domains, and
# `probes` when the corpus was generated with --contradictions.
DOMAINS="$(cd "$CORPUS" && for d in */; do [ -f "$d/MANIFEST.md" ] && printf '%s ' "${d%/}"; done)"

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

# One dotted field out of a JSON document on stdin, or an empty string when it
# is not there.
json_field() {
  python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
path = sys.argv[1].split(".")
for k in path:
    d = d.get(k, {}) if isinstance(d, dict) else {}
print(d if not isinstance(d, dict) else "")' "$1"
}

# One number out of `ctl status --json`, or an empty string when the daemon
# does not answer.
status_field() {
  "$BIN" ctl status --json 2> /dev/null | json_field "$1" || true
}

# The same, read straight off the index file with no daemon involved, for the
# stages that run before one is started.
index_field() {
  "$BIN" status --json --config "$CFG" --db "$DB" 2> /dev/null | json_field "$1" || true
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
  local verify_args=()
  for d in $DOMAINS; do
    verify_args+=("$CORPUS/$d")
  done
  run_step "verify-all" "$BIN" verify "${verify_args[@]}"
  search_battery "search" "text hybrid semantic"
  run_step "evolve" "$BIN" evolve --limit 10 --config "$CFG" --db "$DB"
  run_step "evolve-temporal" \
    "$BIN" evolve --family temporal --limit 10 --config "$CFG" --db "$DB"
  run_step "doctor" "$BIN" doctor --config "$CFG" --db "$DB"
  run_step "reindex-full" "$BIN" reindex --full --config "$CFG" --db "$DB"
}

stage_embed() {
  echo "== embed =="
  # This stage measures a bulk embed, so it has to start from an index that has
  # nothing embedded yet - which is what the base stage leaves behind. A
  # `reindex --full` keeps the embedding of every chunk whose text is unchanged,
  # so over an already-embedded index this stage embeds nothing and reports a
  # handful of seconds that read like a result. Say so instead: the number this
  # stage exists for is the peak resident size of a bulk embed at ten thousand
  # engrams, and a run that measures nothing has to look like a failure.
  local embedded total
  embedded="$(index_field embeddings.embedded_chunks)"
  total="$(index_field embeddings.total_chunks)"
  if [ -n "$embedded" ] && [ -n "$total" ] && [ "$total" != "0" ] && [ "$embedded" = "$total" ]; then
    echo "every chunk is already embedded ($embedded of $total), so this stage would measure nothing." >&2
    echo "run the stages in order on a fresh --out: base, then embed, then daemon." >&2
    return 1
  fi
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

  # Drain whatever embedding backlog is left. In the documented order the embed
  # stage has already done the bulk pass, so this is the daemon's own top-up and
  # usually returns at once; run without that stage it is the bulk pass itself,
  # and the sampler above is what measures it. Either way a backlog that
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

stage_nli() {
  local profile="$NLI_PROFILE" repo
  case "$profile" in
    full) repo="MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7" ;;
    light) repo="MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli" ;;
    english-only) repo="MoritzLaurer/DeBERTa-v3-base-mnli-fever-anli" ;;
    *) echo "--nli-profile must be full, light or english-only" >&2; return 2 ;;
  esac
  if [ "$NLI_LIFT" = "yes" ] && [ ! -f "$CORPUS/contradictions.json" ]; then
    echo "--nli-lift yes measures precision on planted pairs; generate a probes corpus with --contradictions" >&2
    return 2
  fi
  # The lifted pair cap is 20 times the planted pair count rather than a huge
  # constant, so a probes corpus with many engrams cannot still blow up into a
  # quadratic scan; the cap only ever needs to clear "every planted pair plus
  # some slack", not "everything".
  local lift=()
  case "$NLI_LIFT" in
    yes)
      local planted cap
      planted="$(python3 -c 'import json,sys
print(len(json.load(open(sys.argv[1]))))' "$CORPUS/contradictions.json")"
      cap=$((planted * 20))
      lift=(CRYSTALLINE_NLI_RELATED=0.70 "CRYSTALLINE_NLI_MAX_PAIRS=$cap")
      ;;
    no) ;;
    *) echo "--nli-lift must be yes or no" >&2; return 2 ;;
  esac
  echo "== nli ($profile, lift $NLI_LIFT) =="

  # A direct `serve --daemon` never forks: the process invoked below is the
  # daemon itself, running until `ctl shutdown`, and its tracing output goes
  # wherever its own stderr points (the state directory's conventional
  # daemon.log is only ever written by the internal auto-spawn path, which
  # this is not). So this stage owns that path explicitly and starts it fresh,
  # rather than searching for a file a direct invocation never creates.
  local daemon_log="$XDG_STATE_HOME/crystalline/daemon.log"
  mkdir -p "$(dirname "$daemon_log")"
  rm -f "$daemon_log"

  # A model cache of this profile's own, seeded with a clone of the embedding
  # model so the stage measures scoring rather than a download. Checked (and
  # seeded) per checkpoint, not only when the whole directory is missing, so a
  # rerun that reuses an existing $models still gets the NLI checkpoint copied
  # in the first time it is needed. NLI_SEED_MODELS, when set, points at a
  # directory already holding this profile's own checkpoint (the hf-hub cache
  # shape, `models--<org>--<name>`) - without it a first run of a given
  # profile still downloads that one checkpoint, exactly like a real install.
  local models="$OUT/models-$profile"
  mkdir -p "$models"
  local granite_dir
  for granite_dir in "$CRYSTALLINE_MODELS_DIR"/models--ibm-granite--*; do
    [ -d "$granite_dir" ] || continue
    [ -d "$models/$(basename "$granite_dir")" ] || {
      cp -Rc "$granite_dir" "$models/" 2> /dev/null || cp -R "$granite_dir" "$models/"
    }
  done
  if [ -n "$NLI_SEED_MODELS" ]; then
    local hub_name="models--${repo//\//--}"
    if [ -d "$NLI_SEED_MODELS/$hub_name" ] && [ ! -d "$models/$hub_name" ]; then
      cp -Rc "$NLI_SEED_MODELS/$hub_name" "$models/" 2> /dev/null \
        || cp -R "$NLI_SEED_MODELS/$hub_name" "$models/"
    fi
  fi

  local rss="$OUT/nli-$profile-rss.csv" stop="$OUT/.nli-sampler-stop"
  rm -f "$stop"
  echo "seconds,rss_mb,pending,loaded" > "$rss"
  run_step "config-contradictions-$profile" "$BIN" config set evolve.contradictions "$profile" --config "$CFG"

  { env CRYSTALLINE_MODELS_DIR="$models" ${lift[@]+"${lift[@]}"} \
      /usr/bin/time -l "$BIN" serve --daemon --http off --config "$CFG" --db "$DB" \
      > "$LOGS/serve-daemon-nli-$profile.log"; } 2> "$daemon_log" &
  local job=$! waited=0 pid=""
  while [ "$waited" -lt 180 ]; do
    pid="$(status_field pid)"
    [ -n "$pid" ] && break
    kill -0 "$job" 2> /dev/null || break
    sleep 2
    waited=$((waited + 2))
  done
  [ -n "$pid" ] || { echo "the daemon never answered ctl status; see $daemon_log" >&2; return 1; }

  (
    local_elapsed=0
    while [ ! -f "$stop" ]; do
      kill -0 "$pid" 2> /dev/null || break
      size="$(ps -o rss= -p "$pid" 2> /dev/null | tr -d ' ')"
      now_json="$("$BIN" ctl status --json 2> /dev/null || true)"
      pending="$(printf '%s' "$now_json" | json_field contradictions.pending_pairs)"
      loaded="$(printf '%s' "$now_json" | python3 -c 'import json,sys
try: d = json.load(sys.stdin)
except Exception: sys.exit(0)
print("true" if any(a.get("kind") == "contradictions" for a in d.get("activity", {}).get("now", [])) else "false")')"
      printf '%s,%s,%s,%s\n' "$local_elapsed" \
        "$(awk -v k="${size:-0}" 'BEGIN { printf "%.1f", k / 1024 }')" "$pending" "$loaded" >> "$rss"
      sleep "$SAMPLE_EVERY"
      local_elapsed=$((local_elapsed + SAMPLE_EVERY))
    done
  ) &
  local sampler=$!

  # Drained when the daemon reports zero pending after it has reported a
  # number at all (null means no pass ran yet). Once the first pass has logged
  # its speed, project the whole drain (every pending pair at the full 64 line
  # pairs, both orders, batches of 16) and stop early when it cannot finish
  # inside DRAIN_LIMIT: that projection is a result too, and a night is not.
  local elapsed=0 pending checked="" estimate drained_ok=""
  while [ "$elapsed" -lt "$DRAIN_LIMIT" ]; do
    kill -0 "$pid" 2> /dev/null || { echo "the daemon died after ${elapsed}s" >&2; break; }
    pending="$(status_field contradictions.pending_pairs)"
    if [ "$pending" = "0" ]; then
      echo "contradiction backlog drained after about ${elapsed}s"
      drained_ok=1
      break
    fi
    if [ -z "$checked" ] && [ -f "$daemon_log" ] && [ -n "$pending" ] && [ "$pending" != "None" ]; then
      estimate="$(python3 - "$daemon_log" "$pending" <<'PY'
import re, sys
log, pending = sys.argv[1], int(sys.argv[2])
ansi = re.compile(r"\x1b\[[0-9;]*m")
ms = batches = 0
for raw in open(log, encoding="utf-8", errors="replace"):
    line = ansi.sub("", raw)
    if "scored related pairs for possible contradictions" not in line:
        continue
    m_ms = re.search(r"\bms=(\d+)", line)
    m_batches = re.search(r"\bbatches=(\d+)", line)
    if not (m_ms and m_batches):
        continue
    ms += int(m_ms.group(1))
    batches += int(m_batches.group(1))
if batches:
    print(int(pending * 64 * 2 / 16 * ms / batches / 1000))
PY
)"
      if [ -n "$estimate" ]; then
        checked=1
        echo "projected drain: about ${estimate}s for $pending pending pairs"
        if [ "$estimate" -gt "$DRAIN_LIMIT" ]; then
          echo "the projection exceeds DRAIN_LIMIT=${DRAIN_LIMIT}s; stopping. Measure precision on a probes corpus, time the full corpus with --nli-lift no" >&2
          "$BIN" ctl shutdown || true
          touch "$stop"
          wait "$sampler" 2> /dev/null || true
          wait "$job" || true
          return 1
        fi
      fi
    fi
    sleep "$SAMPLE_EVERY"
    elapsed=$((elapsed + SAMPLE_EVERY))
  done
  local drained="$elapsed" fully_drained="false"
  if [ -n "$drained_ok" ]; then
    fully_drained="true"
  else
    echo "the contradiction backlog did not drain (daemon died, or ${DRAIN_LIMIT}s ran out); the dump below is partial" >&2
  fi
  run_step "ctl-status-nli-$profile" "$BIN" ctl status
  echo "sampling ${NLI_UNLOAD_WAIT}s more for the idle drop"
  sleep "$NLI_UNLOAD_WAIT"
  touch "$stop"
  wait "$sampler" 2> /dev/null || true
  run_step "ctl-shutdown-nli-$profile" "$BIN" ctl shutdown
  wait "$job" || true
  [ -f "$daemon_log" ] && cp "$daemon_log" "$LOGS/daemon-nli-$profile.log"

  # Every stored row and every scored pair, with texts and cosines, read
  # straight off the index file once the daemon has let go of it.
  python3 - "$DB" "$repo" "$profile" "$drained" "$OUT/nli-$profile.json" "$fully_drained" <<'PY'
import json, sqlite3, sys
db, repo, profile, drained, out, fully_drained = sys.argv[1:7]
con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
rows = con.execute(
    """SELECT d.name, ea.permalink, eb.permalink, c.line_a, c.line_b, c.score_ab, c.score_ba,
              c.period, p.cosine, oa.content, ob.content
       FROM contradiction c
       JOIN contradiction_pair p ON p.engram_a = c.engram_a AND p.engram_b = c.engram_b AND p.model = c.model
       JOIN engram ea ON ea.id = c.engram_a
       JOIN engram eb ON eb.id = c.engram_b
       JOIN domain d ON d.id = c.domain_id
       LEFT JOIN observation oa ON oa.engram_id = c.engram_a AND oa.line = c.line_a
       LEFT JOIN observation ob ON ob.engram_id = c.engram_b AND ob.line = c.line_b
       WHERE c.model = ?""",
    (repo,),
).fetchall()
pairs = con.execute(
    """SELECT d.name, ea.permalink, eb.permalink, p.cosine FROM contradiction_pair p
       JOIN engram ea ON ea.id = p.engram_a JOIN engram eb ON eb.id = p.engram_b
       JOIN domain d ON d.id = p.domain_id WHERE p.model = ?""",
    (repo,),
).fetchall()
keys = ["domain", "a", "b", "line_a", "line_b", "score_ab", "score_ba", "period", "cosine", "a_text", "b_text"]
json.dump(
    {
        "profile": profile,
        "model": repo,
        "wall_seconds": int(drained),
        "drained": fully_drained == "true",
        "rows": [dict(zip(keys, r)) for r in rows],
        "pairs": [dict(zip(["domain", "a", "b", "cosine"], p)) for p in pairs],
    },
    open(out, "w", encoding="utf-8"),
    ensure_ascii=False,
    indent=1,
)
note = "" if fully_drained == "true" else " (PARTIAL: the backlog had not drained)"
print(f"dumped {len(rows)} rows over {len(pairs)} scored pairs to {out}{note}")
PY
  printf '%s,%s,%s,%s,%s\n' "$STAGE" "nli-$profile-drain" 0 "$drained" \
    "$(awk -F, 'NR > 1 && $2 != "" && $2 + 0 > m { m = $2 + 0 } END { printf "%.1f", m }' "$rss")" >> "$CSV"
}

case "$STAGE" in
  base) stage_base ;;
  embed) stage_embed ;;
  daemon) stage_daemon ;;
  nli) stage_nli ;;
  all) stage_base; stage_embed; stage_daemon ;;
esac

echo
echo "results: $CSV"
cat "$CSV"
