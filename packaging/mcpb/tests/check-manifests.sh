#!/usr/bin/env bash
# Generates both bundle manifests into a scratch folder and checks the facts
# each one must carry. Run from anywhere; CI runs it in the mcpb job.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
gen="$here/../generate-manifest.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail() {
    echo "check-manifests: $1" >&2
    exit 1
}

"$gen" standard 9.9.9 "$tmp/standard" >/dev/null
m="$tmp/standard/manifest.json"
[ "$(jq -r .server.type "$m")" = node ] || fail "standard: server.type is not node"
[ "$(jq -r .server.entry_point "$m")" = server/shim.js ] || fail "standard: entry_point"
[ "$(jq -r .server.mcp_config.command "$m")" = node ] || fail "standard: command"
# Literal placeholder for the host to expand, not a shell variable.
# shellcheck disable=SC2016
[ "$(jq -c .server.mcp_config.args "$m")" = '["${__dirname}/server/shim.js"]' ] || fail "standard: args"
[ "$(jq -c .compatibility.platforms "$m")" = '["darwin","win32"]' ] || fail "standard: platforms"
[ "$(jq -r '.server.mcp_config.platform_overrides // "none"' "$m")" = none ] || fail "standard: overrides"
[ "$(jq -r '.server.mcp_config.env // "none"' "$m")" = none ] || fail "standard: the shim sets its own channel"
[ "$(jq -r .version "$m")" = 9.9.9 ] || fail "standard: version"
[ "$(jq -r .display_name "$m")" = Crystalline ] || fail "standard: display_name"
[ "$(jq -r '.compatibility.runtimes // "none"' "$m")" = none ] || fail "standard: declares runtimes"
cmp -s "$tmp/standard/server/shim.js" "$here/../shim/shim.js" || fail "standard: shim.js is not staged"
[ -f "$tmp/standard/icon.png" ] || fail "standard: icon"
[ ! -e "$tmp/standard/server/crystalline" ] || fail "standard: carries a binary"

# The launcher reads its version from manifest.json one folder above itself
# and falls back to 0.24.0 without a word. So the staged launcher, loaded
# from its staged place, must read the staged manifest's 9.9.9. Loading it
# runs nothing: main() starts only when the file is run itself.
shim_version="$(node -e 'process.stdout.write(String(require(process.argv[1]).extensionVersion()))' "$tmp/standard/server/shim.js")" \
    || fail "standard: the staged shim.js does not load"
[ "$shim_version" = 9.9.9 ] || fail "standard: the staged shim.js reads version '$shim_version', not the staged manifest's 9.9.9"

"$gen" desktop-only 9.9.9 "$tmp/desktop" >/dev/null
d="$tmp/desktop/manifest.json"
[ "$(jq -r .server.type "$d")" = binary ] || fail "desktop-only: server.type is not binary"
[ "$(jq -r .server.entry_point "$d")" = server/crystalline ] || fail "desktop-only: entry_point"
# shellcheck disable=SC2016
[ "$(jq -r .server.mcp_config.command "$d")" = '${__dirname}/server/crystalline' ] || fail "desktop-only: command"
[ "$(jq -c .server.mcp_config.args "$d")" = '["mcp"]' ] || fail "desktop-only: args"
[ "$(jq -r .server.mcp_config.env.CRYSTALLINE_CHANNEL "$d")" = mcpb ] || fail "desktop-only: channel"
[ "$(jq -c .compatibility.platforms "$d")" = '["darwin"]' ] || fail "desktop-only: platforms"
[ "$(jq -r .display_name "$d")" = "Crystalline (Claude Desktop only)" ] || fail "desktop-only: display_name"
[ "$(jq -r '.compatibility.runtimes // "none"' "$d")" = none ] || fail "desktop-only: declares runtimes"
[ "$(jq -r .name "$d")" = "$(jq -r .name "$m")" ] || fail "both bundles are one extension to Claude Desktop"
[ ! -e "$tmp/desktop/server/shim.js" ] || fail "desktop-only: carries the shim"

if "$gen" macos-arm64 9.9.9 "$tmp/old" >/dev/null 2>&1; then
    fail "the per-arch bundles are gone"
fi
echo "check-manifests: both manifests ok"
