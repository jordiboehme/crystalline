#!/usr/bin/env bash
# Run the Fluid browser smoke against a real Crystalline daemon.
#
# One command, the same one locally and in CI: it stands up a daemon holding a
# copy of the fixture domain, creates the admin account through the first-run
# setup endpoint the browser wizard drives and seeds a peer editor with the CLI,
# checks the web UI that daemon serves out of its own binary, then builds the
# bundle and hands both to Playwright, which serves the bundle with `vite
# preview` (see playwright.config.ts) and drives a browser against it.
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
# The daemon takes port 7411 when it is free and the next free port otherwise,
# and hands the address to vite through CRYSTALLINE_API_TARGET so the /api proxy
# in front of the browser journeys follows it. FLUID_E2E_PORT pins a port
# instead.
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

# Where the daemon listens. 7411 is Crystalline's own default and stays the
# first choice, but it is no longer a port this script may assume is free: the
# HTTP endpoint is on by default now, so any daemon on the machine - a
# hand-started `crystalline serve`, or one an agent session spawned - already
# holds it. A run that quietly probed THAT daemon would be checking the
# developer's own instance instead of the scratch one it just built, and the
# bind failure a busy port produces is deliberately non-fatal for the daemon, so
# nothing would say so. Stepping aside to a free port is the whole fix; the
# address travels to `vite preview` through CRYSTALLINE_API_TARGET (see
# ../vite.config.ts), which is what keeps the browser journeys pointed at it.
port_is_free() {
    # A refused connect is a free port. The probe runs in a subshell so the
    # descriptor it may open dies with it, and it reserves nothing: a port that
    # is taken between here and the daemon's own bind surfaces as the
    # bind-failure check in the readiness loop below.
    if (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; then
        return 1
    fi
    return 0
}

DAEMON_PORT="${FLUID_E2E_PORT:-}"
if [ -z "$DAEMON_PORT" ]; then
    for candidate in $(seq 7411 7431); do
        if port_is_free "$candidate"; then
            DAEMON_PORT="$candidate"
            break
        fi
    done
fi
if [ -z "$DAEMON_PORT" ]; then
    echo "smoke: every port from 7411 to 7431 is taken; free one or set FLUID_E2E_PORT" >&2
    exit 1
fi
DAEMON_ADDR="127.0.0.1:$DAEMON_PORT"
export CRYSTALLINE_API_TARGET="http://$DAEMON_ADDR"

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

# The daemon comes up before any account exists, which is the state a real
# first run is in: the admin below is created through the daemon's own setup
# endpoint rather than seeded past it.
#
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
    # A busy port does not stop the daemon: it keeps serving MCP over its socket
    # and warns about the endpoint. Without this check the healthcheck below
    # would then be answered by whatever else holds the port, and the whole run
    # would assert against a daemon this script never started.
    if grep -q 'HTTP endpoint failed on' "$run_dir/daemon.log" 2>/dev/null; then
        echo "smoke: the daemon could not open the HTTP endpoint on $DAEMON_ADDR" >&2
        grep 'HTTP endpoint failed on' "$run_dir/daemon.log" >&2
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

# The first admin, created the way a person creates one: through the endpoint
# behind the browser's first-run wizard, over loopback, where no setup token is
# asked for (a non-loopback bind is the case that prints one, and this daemon
# binds 127.0.0.1). The component tests cover the form itself; what only a real
# daemon can answer is the endpoint's own story, so this block walks it: the
# probe advertises the open slot, the POST creates the admin and signs it in,
# the probe closes, and the slot is gone for everyone after.
setup_headers="$run_dir/setup-headers"
setup_body="$run_dir/setup-body"

# The response body and headers land in the two files above; the status code is
# printed, as in the UI block further down.
api_get() {
    curl --silent --show-error --output "$setup_body" --dump-header "$setup_headers" \
        --write-out '%{http_code}' --max-time 30 \
        --header 'Accept: application/json' "http://$DAEMON_ADDR$1"
}

api_post_json() {
    curl --silent --show-error --output "$setup_body" --dump-header "$setup_headers" \
        --write-out '%{http_code}' --max-time 30 \
        --header 'Content-Type: application/json' --data "$2" \
        "http://$DAEMON_ADDR$1"
}

api_fail() {
    echo "smoke: $1" >&2
    echo "smoke: the response body follows" >&2
    cat "$setup_body" >&2
    echo >&2
    exit 1
}

echo "smoke: creating the first admin through the setup endpoint"

status=$(api_get /api/v1/auth/me)
if [ "$status" != "200" ]; then
    api_fail "GET /api/v1/auth/me answered $status rather than 200; the capability probe is public by design"
fi
grep -q '"needs_setup":[[:space:]]*true' "$setup_body" \
    || api_fail "a daemon with no accounts does not report needs_setup, so the browser would render a login form nobody can use"

# The credentials are the ones every journey logs in with. Spelled into JSON by
# hand because they are plain by construction; a value carrying a quote would
# need a real encoder here.
status=$(api_post_json /api/v1/auth/setup \
    "$(printf '{"name":"%s","password":"%s"}' "$FLUID_E2E_USER" "$FLUID_E2E_PASSWORD")")
if [ "$status" != "200" ]; then
    api_fail "POST /api/v1/auth/setup answered $status rather than 200 for a loopback caller on an instance with no accounts"
fi
tr -d '\r' < "$setup_headers" | grep -qi '^set-cookie:.*fluid_session' \
    || api_fail "the setup answer carries no fluid_session cookie, so the wizard would create an admin and leave them logged out"

status=$(api_get /api/v1/auth/me)
grep -q '"needs_setup":[[:space:]]*false' "$setup_body" \
    || api_fail "the probe still reports needs_setup after the admin was created ($status), so the wizard would never make way for the login form"

# Once is the whole contract: the slot is permanently gone rather than merely
# guarded, so a second caller is refused whoever they are.
status=$(api_post_json /api/v1/auth/setup \
    "$(printf '{"name":"%s","password":"%s"}' "second-admin" "second-password")")
if [ "$status" != "410" ]; then
    api_fail "a second POST /api/v1/auth/setup answered $status rather than 410; the first-run slot has to close for good"
fi

# An editor rather than a second admin: the co-editing journey only needs to
# reach the editor, and a peer with no more rights than that is the account a
# real second author would have. The CLI is the right tool for it and the path
# an operator scripts, so the run covers that half too: `users add` writes the
# accounts database directly, and the running daemon picks the account up on
# its next lookup.
echo "smoke: seeding the peer account"
printf '%s' "$FLUID_E2E_PEER_PASSWORD" \
    | "${isolated[@]}" "$bin" users add "$FLUID_E2E_PEER" --role editor --password-stdin

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
