#!/usr/bin/env bash
#
# Writes a version into the workspace Cargo.toml and fluid/package.json of a
# build checkout, so the binary and the web UI it embeds carry it:
# `crystalline --version`, the MCP server info, the ETag suffix and the Fluid
# about line all read it from there at compile time.
#
# For CI checkouts only, and never committed. .github/workflows/build.yml runs
# it for a dev-channel build, with the version scripts/dev-version.sh computed;
# the stable release builds what the tag says and never calls this. The cargo
# target rewrites the workspace entries of Cargo.lock too, because the builds
# run with --locked. Both files
# are rewritten together because Fluid compares its own version against the
# server's and would warn about a version skew that does not exist.
#
#   bash scripts/set-build-version.sh <version> [cargo|fluid|all]
#
# The second argument picks the file: the fluid bundle is built in a job of its
# own, so each job rewrites only the file it builds from. Default: all.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="${1:-}"
which="${2:-all}"

# A semantic version with an optional pre-release; Cargo rejects anything else
# and the Homebrew formula's test compares it against --version.
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "set-build-version: '$version' is not a version like 0.21.0 or 0.21.0-dev.4817" >&2
  exit 1
fi

set_cargo() {
  local file="$root/Cargo.toml"
  # Only the version line of the [workspace.package] table: the first
  # `version = "..."` after that header, before the next table starts.
  VERSION="$version" perl -0pi -e \
    's/(\[workspace\.package\]\n(?:(?!\[)[^\n]*\n)*?)version = "[^"]*"/$1version = "$ENV{VERSION}"/' \
    "$file"
  local now
  now="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$file" | head -1)"
  if [ "$now" != "$version" ]; then
    echo "set-build-version: Cargo.toml still says '$now' after the rewrite" >&2
    exit 1
  fi
  echo "Cargo.toml: $now"

  # The release and dev builds run `cargo build --locked`, which refuses a
  # lockfile that disagrees with Cargo.toml. The workspace crates are the
  # lockfile entries named crystalline or crystalline-<name> with no
  # `source` line (a registry crate always has one), so only their version
  # moves; no dependency is re-resolved.
  local lock="$root/Cargo.lock"
  VERSION="$version" perl -0pi -e \
    's/(\[\[package\]\]\nname = "crystalline(?:-[a-z]+)?"\nversion = )"[^"]*"(\n(?!source = ))/$1"$ENV{VERSION}"$2/g' \
    "$lock"
  local members moved
  members="$(grep -cE '^name = "crystalline(-[a-z]+)?"$' "$lock" || true)"
  moved="$(grep -A1 -E '^name = "crystalline(-[a-z]+)?"$' "$lock" | grep -cx "version = \"$version\"" || true)"
  if [ "$members" = "0" ] || [ "$members" != "$moved" ]; then
    echo "set-build-version: Cargo.lock has $members workspace entries and $moved of them say '$version'" >&2
    exit 1
  fi
  echo "Cargo.lock: $moved workspace entries at $version"
}

set_fluid() {
  local file="$root/fluid/package.json"
  # The first "version" key is the package's own, two spaces in.
  VERSION="$version" perl -0pi -e \
    's/^(  "version": )"[^"]*"/$1"$ENV{VERSION}"/m' \
    "$file"
  local now
  now="$(sed -n 's/^  "version": "\(.*\)".*/\1/p' "$file" | head -1)"
  if [ "$now" != "$version" ]; then
    echo "set-build-version: fluid/package.json still says '$now' after the rewrite" >&2
    exit 1
  fi
  echo "fluid/package.json: $now"
}

case "$which" in
  cargo) set_cargo ;;
  fluid) set_fluid ;;
  all) set_cargo; set_fluid ;;
  *)
    echo "set-build-version: unknown target '$which' (cargo, fluid or all)" >&2
    exit 1
    ;;
esac
