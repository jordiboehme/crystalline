#!/usr/bin/env bash
# Run the Fluid browser smoke against a real Crystalline daemon.
#
# One command, the same one locally and in CI: it builds the bundle, stands up
# a daemon holding a copy of the fixture domain and one seeded account, and
# hands both to Playwright, which serves the bundle with `vite preview` (see
# playwright.config.ts) and drives a browser against it.
#
# Everything the daemon writes lives in a scratch directory this script makes
# and removes: the config, the index, the accounts, the model cache and the
# copy of the fixture domain the daemon indexes and writes its generated index
# files into. Nothing here touches the machine's own Crystalline installation,
# and the checked-in fixture under e2e/fixtures/domain is never written to.
#
# The daemon's port is 7411 and not configurable, because vite.config.ts
# forwards /api there and the browser only ever talks to the preview server.
#
#   bash fluid/e2e/run-smoke.sh                 # the whole suite
#   bash fluid/e2e/run-smoke.sh --headed        # arguments reach playwright
#
# CRYSTALLINE_BIN names the binary to serve with; it defaults to the debug
# build, then the release build, and says so when it finds neither.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fluid_dir="$(dirname "$here")"
repo_root="$(dirname "$fluid_dir")"

DAEMON_ADDR="127.0.0.1:7411"
FLUID_E2E_USER="${FLUID_E2E_USER:-smoke}"
FLUID_E2E_PASSWORD="${FLUID_E2E_PASSWORD:-smoke-password}"
FLUID_E2E_DOMAIN="${FLUID_E2E_DOMAIN:-fluid-smoke}"
export FLUID_E2E_USER FLUID_E2E_PASSWORD FLUID_E2E_DOMAIN

bin="${CRYSTALLINE_BIN:-}"
if [ -z "$bin" ]; then
    for candidate in "$repo_root/target/debug/crystalline" "$repo_root/target/release/crystalline"; do
        if [ -x "$candidate" ]; then
            bin="$candidate"
            break
        fi
    done
fi
if [ -z "$bin" ] || [ ! -x "$bin" ]; then
    echo "smoke: no crystalline binary; run 'cargo build -p crystalline' or set CRYSTALLINE_BIN" >&2
    exit 1
fi

run_dir="$(mktemp -d)"
daemon_pid=""

finish() {
    status=$?
    # A clean stop first, then a hard one. The daemon fetches the embedding
    # model on a first run and its runtime waits for that download before it
    # exits, which is right for a real installation and only a delay here: this
    # state directory is about to be deleted, so a few seconds is all the
    # courtesy it gets.
    if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
        kill "$daemon_pid" 2>/dev/null || true
        for _ in $(seq 1 10); do
            kill -0 "$daemon_pid" 2>/dev/null || break
            sleep 1
        done
        kill -9 "$daemon_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    # The daemon's own account of the run, but only when something failed: it
    # is the first place to look and the last thing anyone wants in a green log.
    if [ "$status" -ne 0 ] && [ -f "$run_dir/daemon.log" ]; then
        echo "smoke: the last of the daemon log follows" >&2
        tail -n 40 "$run_dir/daemon.log" >&2 || true
    fi
    rm -rf "$run_dir"
    exit "$status"
}
trap finish EXIT

# Every path the daemon resolves, pointed inside the scratch directory. The
# config, state and cache directories all come from these (see
# crates/core/src/config.rs), so this is the whole of the isolation.
#
# Handed to the Crystalline commands one at a time rather than exported into
# this shell, and that is load bearing. XDG_CACHE_HOME is not Crystalline's
# variable: on Linux it is where Playwright keeps its browsers
# (`XDG_CACHE_HOME || ~/.cache` then `ms-playwright`, in playwright-core's
# registry), so exporting it pointed `playwright test` at a scratch directory
# holding nothing but an embedding model, and every test failed with a browser
# it had just installed into the real cache moments earlier. macOS never showed
# it: Playwright reads ~/Library/Caches there and ignores XDG entirely. Scoping
# the isolation to the process it belongs to is what makes that impossible
# rather than merely fixed, for pnpm's store and anything else reading XDG too.
isolated=(
    env
    "XDG_CONFIG_HOME=$run_dir/config"
    "XDG_STATE_HOME=$run_dir/state"
    "XDG_CACHE_HOME=$run_dir/cache"
    "XDG_DATA_HOME=$run_dir/data"
)
mkdir -p "$run_dir/config" "$run_dir/state" "$run_dir/cache" "$run_dir/data"

domain_root="$run_dir/domain"
cp -R "$here/fixtures/domain" "$domain_root"

echo "smoke: registering the fixture domain"
"${isolated[@]}" "$bin" domain add "$FLUID_E2E_DOMAIN" "$domain_root"

echo "smoke: seeding the account"
printf '%s' "$FLUID_E2E_PASSWORD" \
    | "${isolated[@]}" "$bin" users add "$FLUID_E2E_USER" --role admin --password-stdin

# `env` execs, so the pid recorded here is the daemon's own and the trap above
# signals the daemon rather than a wrapper around it.
echo "smoke: starting the daemon on $DAEMON_ADDR"
"${isolated[@]}" "$bin" serve --http "$DAEMON_ADDR" > "$run_dir/daemon.log" 2>&1 &
daemon_pid=$!

# The same probe an external monitor makes, so a daemon that answers here is a
# daemon the proxy in front of it can reach.
ready=0
for _ in $(seq 1 60); do
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
        echo "smoke: the daemon exited before it was ready" >&2
        exit 1
    fi
    if "${isolated[@]}" "$bin" healthcheck "$DAEMON_ADDR" > /dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 1
done
if [ "$ready" -ne 1 ]; then
    echo "smoke: the daemon never became healthy on $DAEMON_ADDR" >&2
    exit 1
fi

cd "$fluid_dir"

# The bundle under test is built here rather than assumed: `vite preview` serves
# whatever is in dist/, and a stale one would be a green run of last week's app.
echo "smoke: building the bundle"
pnpm build

# Where this run expects to find a browser, said out loud before it needs one.
# A missing browser otherwise reports only that an executable is not at a path,
# and the useful half of that story is which path was resolved and why.
browsers=$(pnpm exec playwright install --dry-run chromium 2>/dev/null \
    | awk '/Install location/ { print $3; exit }')
echo "smoke: playwright browsers at ${browsers:-an unknown location}"

echo "smoke: running playwright"
pnpm exec playwright test "$@"
