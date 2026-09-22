#!/usr/bin/env bash
#
# The version moves three files together, and CI only ever checked two of them.
#
# The workspace Cargo.toml is the authority. fluid/package.json is compared
# against it by the `fluid` job (the UI warns a reader about a version skew when
# the two disagree), and docs/evolve.md carries three pins nothing read at all:
# the action ref users copy into their workflow, the binary version that
# workflow downloads, and the prose that tells them pinning both is what makes
# the check reproducible. A bump that updates two files of the three goes green
# and ships, and every reader who copies the snippet pins the previous release.
#
# So this checks all three, and fails when a pin is missing as loudly as when
# one is wrong: a docs rewrite that drops a pin must not read as agreement.
#
#   bash scripts/version-pins.sh
#
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
pinfile="$root/docs/evolve.md"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -1)"
if [ -z "$version" ]; then
  echo "::error file=Cargo.toml::no workspace version found in Cargo.toml" >&2
  exit 1
fi

# The three pin forms, in the order docs/evolve.md carries them: the action ref,
# the binary version input and the prose that names the ref again.
pins="$(grep -E -o \
  'crystalline/action@v[0-9]+\.[0-9]+\.[0-9]+|version: v[0-9]+\.[0-9]+\.[0-9]+|`@v[0-9]+\.[0-9]+\.[0-9]+`' \
  "$pinfile" || true)"
count="$(printf '%s' "$pins" | grep -c . || true)"

if [ "$count" != "3" ]; then
  echo "::error file=docs/evolve.md::expected 3 version pins in docs/evolve.md (the action ref, the version input and the prose ref), found $count. The snippet users copy is what pins their workflow, so a dropped pin is not agreement." >&2
  printf '%s\n' "$pins" >&2
  exit 1
fi

bad=0
while read -r pin; do
  [ -n "$pin" ] || continue
  pinned="$(printf '%s' "$pin" | sed -E 's/.*@?v([0-9]+\.[0-9]+\.[0-9]+).*/\1/')"
  if [ "$pinned" != "$version" ]; then
    echo "::error file=docs/evolve.md::docs/evolve.md pins $pin while the workspace Cargo.toml says $version. The version moves three files together: Cargo.toml, docs/evolve.md and fluid/package.json." >&2
    bad=1
  fi
done <<< "$pins"

fluid="$(sed -n 's/.*"version": "\(.*\)".*/\1/p' "$root/fluid/package.json" | head -1)"
if [ "$fluid" != "$version" ]; then
  echo "::error file=fluid/package.json::fluid/package.json says $fluid and the workspace Cargo.toml says $version. Fluid compares its own version against the server's and warns the reader when they differ, so the two are bumped together." >&2
  bad=1
fi

[ "$bad" = "0" ] || exit 1
echo "version pins: $version (Cargo.toml, 3 pins in docs/evolve.md, fluid/package.json)"
