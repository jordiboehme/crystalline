#!/usr/bin/env bash
# Run the Fluid browser smoke against a real Crystalline daemon.
#
# One command, the same one locally and in CI: it stands up a daemon holding a
# copy of the fixture domain and two seeded accounts, checks the web UI that
# daemon serves out of its own binary, then builds the bundle and hands both to
# Playwright, which serves the bundle with `vite preview` (see
# playwright.config.ts) and drives a browser against it.
#
# The two deployments are both real and both covered: the embedded one the
# curl block asserts (the bundle compiled into the binary, served by
# `serve --http`) and the compose one Playwright drives (a separate server in
# front of the daemon). The embedded checks need a binary built AFTER
# `pnpm --dir fluid build`, since the bundle is staged at compile time.
#
# Everything the daemon writes lives in a scratch directory this script makes
# and removes: the config, the index, the accounts, the model cache, the copy
# of the fixture domain the daemon indexes and writes its generated index files
# into, and the domains root a domain registered from the app lands under.
# Nothing here touches the machine's own Crystalline installation, and the
# checked-in fixture under e2e/fixtures/domain is never written to.
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
# The second account, for the two-browser co-editing journey: a room needs two
# people, and two people in one room need two logins.
FLUID_E2E_PEER="${FLUID_E2E_PEER:-peer}"
FLUID_E2E_PEER_PASSWORD="${FLUID_E2E_PEER_PASSWORD:-peer-password}"
export FLUID_E2E_USER FLUID_E2E_PASSWORD FLUID_E2E_DOMAIN
export FLUID_E2E_PEER FLUID_E2E_PEER_PASSWORD

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
#
# The domains root is the one path here that is NOT an XDG one: it defaults to
# ~/Documents/Crystalline, in the daemon's own home, and it is where a local
# domain registered from the app lands. The domain-administration journey
# registers one, so without this line a smoke run would leave a real folder in
# the developer's Documents rather than in the scratch directory the trap above
# removes.
isolated=(
    env
    "XDG_CONFIG_HOME=$run_dir/config"
    "XDG_STATE_HOME=$run_dir/state"
    "XDG_CACHE_HOME=$run_dir/cache"
    "XDG_DATA_HOME=$run_dir/data"
    "CRYSTALLINE_DOMAINS_ROOT=$run_dir/domains-root"
)
mkdir -p "$run_dir/config" "$run_dir/state" "$run_dir/cache" "$run_dir/data" \
    "$run_dir/domains-root"

domain_root="$run_dir/domain"
cp -R "$here/fixtures/domain" "$domain_root"

echo "smoke: registering the fixture domain"
"${isolated[@]}" "$bin" domain add "$FLUID_E2E_DOMAIN" "$domain_root"

echo "smoke: seeding the account"
printf '%s' "$FLUID_E2E_PASSWORD" \
    | "${isolated[@]}" "$bin" users add "$FLUID_E2E_USER" --role admin --password-stdin

# An editor rather than a second admin: the co-editing journey only needs to
# reach the editor, and a peer with no more rights than that is the account a
# real second author would have.
echo "smoke: seeding the peer account"
printf '%s' "$FLUID_E2E_PEER_PASSWORD" \
    | "${isolated[@]}" "$bin" users add "$FLUID_E2E_PEER" --role editor --password-stdin

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

# The embedded web UI, checked against the daemon's own port before the browser
# journeys start. Playwright drives `vite preview` (the compose scenario, where
# nginx serves the bundle), so this block is the only place the run exercises
# the other deployment: the bundle compiled into the binary and served by
# `serve --http` itself. It runs early because it is cheap and because a UI-less
# binary is worth hearing about before a browser download.
#
# The four checks below are the contract: the shell is served with no-store, a
# hashed asset is immutable, a data route without a cookie is refused, and the
# MCP standby stream at `/` is never answered with the shell.
echo "smoke: checking the embedded web UI on http://$DAEMON_ADDR"

ui_headers="$run_dir/ui-headers"
ui_body="$run_dir/ui-body"

# One GET, with the Accept header the caller cares about. Prints the status
# code; the headers and the body stay in the two files above so the assertions
# after it can look at either.
ui_get() {
    curl --silent --show-error --output "$ui_body" --dump-header "$ui_headers" \
        --write-out '%{http_code}' --max-time 30 \
        --header "Accept: $2" "http://$DAEMON_ADDR$1"
}

# Header names are case insensitive and every value here is ASCII, so both sides
# are lowercased once and the match is a plain substring. The trailing CR the
# protocol puts on each line would otherwise end up inside the value.
ui_header_has() {
    tr -d '\r' < "$ui_headers" | tr '[:upper:]' '[:lower:]' | grep -q "^$1:.*$2"
}

ui_fail() {
    echo "smoke: $1" >&2
    echo "smoke: the response headers follow" >&2
    cat "$ui_headers" >&2
    exit 1
}

status=$(ui_get / 'text/html,application/xhtml+xml')
if [ "$status" != "200" ]; then
    ui_fail "GET / answered $status rather than 200. A binary compiled with no bundle staged answers 503 here: run 'pnpm --dir fluid build' and rebuild the binary (the build script copies fluid/dist at compile time, so the bundle has to exist first)."
fi
ui_header_has content-type 'text/html' \
    || ui_fail "GET / is not text/html, so the daemon did not serve the app shell"
ui_header_has cache-control 'no-store' \
    || ui_fail "GET / is not no-store; the one unhashed name must never be cached"

# The asset is read out of the shell the daemon just served rather than off
# disk, so it names a file THIS binary carries. dist/ is rebuilt further down
# for `vite preview`, and a rebuild that lands different hashes than the ones
# compiled in would otherwise fail this check for no fault of the server.
asset=$(grep -o '/assets/[A-Za-z0-9._-]*' "$ui_body" | head -n 1 || true)
if [ -z "$asset" ]; then
    ui_fail "the served shell references no /assets/ file, so there is no hashed asset to check"
fi
status=$(ui_get "$asset" '*/*')
if [ "$status" != "200" ]; then
    ui_fail "GET $asset answered $status rather than 200, though the shell the same binary served asks for it"
fi
ui_header_has cache-control 'immutable' \
    || ui_fail "GET $asset is not immutable; hashed assets carry a one year cache"

# A data route, not /api/v1/auth/me: that one is public by design (it is how a
# logged-out client learns it must log in). This daemon has accounts and no
# anonymous access, so the API refuses an uncredentialed reader; with
# auth.anonymous=true the same request would be 200 at viewer level.
status=$(ui_get /api/v1/domains 'application/json')
if [ "$status" != "401" ]; then
    ui_fail "GET /api/v1/domains without a cookie answered $status rather than 401; serving the UI must not open the API"
fi

# The MCP transport's standby stream: a client opens it with GET / and
# `Accept: text/event-stream`, and it is the channel server notifications ride.
# Answering it with the app shell breaks every stateful MCP session, so the
# shape asserted here is the transport's own refusal of a session-less stream
# (400) rather than anything the UI could produce.
status=$(ui_get / 'text/event-stream')
if ui_header_has content-type 'text/html'; then
    ui_fail "GET / with Accept: text/event-stream was answered with the app shell; the MCP standby stream has to reach the transport"
fi
case "$status" in
    400 | 406) ;;
    *) ui_fail "GET / with Accept: text/event-stream answered $status; the transport refuses a session-less stream with 400 or 406" ;;
esac

echo "smoke: the embedded UI serves the shell, a hashed asset and no data"

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
