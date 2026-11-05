#!/usr/bin/env bash
# Stages both bundles with `crystalline desktop-bundle` into a scratch folder
# and checks the facts each one must carry. Needs a built crystalline in
# CRYSTALLINE_BIN, plus jq, unzip and node. CI runs it in the mcpb job and
# the release runs it in its own mcpb job.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail() {
    echo "check-manifests: $1" >&2
    exit 1
}

[ -n "${CRYSTALLINE_BIN:-}" ] && [ -x "$CRYSTALLINE_BIN" ] || fail "set CRYSTALLINE_BIN to a built crystalline"

# The first line only: --version prints the copyright and source below it.
version="$("$CRYSTALLINE_BIN" --version | head -n1 | awk '{print $2}')"
[ -n "$version" ] || fail "could not read the version from $CRYSTALLINE_BIN --version"

"$CRYSTALLINE_BIN" desktop-bundle hollow "$tmp/hollow" --binary /opt/homebrew/bin/crystalline --pack "$tmp/hollow.mcpb" >/dev/null
m="$tmp/hollow/manifest.json"
[ "$(jq -r .server.type "$m")" = node ] || fail "hollow: server.type is not node"
[ "$(jq -r .server.entry_point "$m")" = server/shim.js ] || fail "hollow: entry_point"
[ "$(jq -r .server.mcp_config.command "$m")" = node ] || fail "hollow: command"
# Literal placeholder for the host to expand, not a shell variable.
# shellcheck disable=SC2016
[ "$(jq -c .server.mcp_config.args "$m")" = '["${__dirname}/server/shim.js"]' ] || fail "hollow: args"
[ "$(jq -c .compatibility.platforms "$m")" = '["darwin","win32"]' ] || fail "hollow: platforms"
[ "$(jq -r '.server.mcp_config.platform_overrides // "none"' "$m")" = none ] || fail "hollow: overrides"
[ "$(jq -c .server.mcp_config.env "$m")" = '{"CRYSTALLINE_BINARY":"/opt/homebrew/bin/crystalline"}' ] || fail "hollow: the pinned binary"
[ "$(jq -r .version "$m")" = "$version" ] || fail "hollow: version is not $version"
[ "$(jq -r .display_name "$m")" = Crystalline ] || fail "hollow: display_name"
[ "$(jq -r '.compatibility.runtimes // "none"' "$m")" = none ] || fail "hollow: declares runtimes"
cmp -s "$tmp/hollow/server/shim.js" "$here/../shim/shim.js" || fail "hollow: shim.js is not staged"
[ -f "$tmp/hollow/icon.png" ] || fail "hollow: icon"
[ ! -e "$tmp/hollow/server/crystalline" ] || fail "hollow: carries a binary"

# The launcher reads its version from manifest.json one folder above itself
# and falls back to 0.24.0 without a word. So the staged launcher, loaded
# from its staged place, must read the staged manifest's version. Loading it
# runs nothing: main() starts only when the file is run itself. The path
# travels in the environment, not in argv: the launcher takes argv[1] naming
# itself for "run as the entry" and would start the installed crystalline.
shim_version="$(SHIM="$tmp/hollow/server/shim.js" node -e 'process.stdout.write(String(require(process.env.SHIM).extensionVersion()))' </dev/null)" \
    || fail "hollow: the staged shim.js does not load"
[ "$shim_version" = "$version" ] || fail "hollow: the staged shim.js reads version '$shim_version', not the staged manifest's $version"

# The packed bundle holds exactly these three files.
[ "$(unzip -Z1 "$tmp/hollow.mcpb" | sort | tr '\n' ' ')" = "icon.png manifest.json server/shim.js " ] || fail "hollow: the packed bundle does not hold exactly icon.png, manifest.json and server/shim.js"

"$CRYSTALLINE_BIN" desktop-bundle desktop-only "$tmp/desktop" >/dev/null
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
[ "$(jq -r .version "$d")" = "$version" ] || fail "desktop-only: version is not $version"
[ "$(jq -r .name "$d")" = "$(jq -r .name "$m")" ] || fail "both bundles are one extension to Claude Desktop"
[ ! -e "$tmp/desktop/server/shim.js" ] || fail "desktop-only: carries the shim"

echo "check-manifests: both manifests ok"
