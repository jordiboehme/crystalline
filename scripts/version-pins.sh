#!/usr/bin/env bash
#
# The version moves four files together, and CI used to check only two of them.
#
# The workspace Cargo.toml is the authority. fluid/package.json is compared
# against it by the `fluid` job (the UI warns a reader about a version skew when
# the two disagree), docs/evolve.md carries three pins nothing read at all:
# the action ref users copy into their workflow, the binary version that
# workflow downloads, and the prose that tells them pinning both is what makes
# the check reproducible, and release-notes/v<version>.md is the file the
# tagged release workflow publishes as the release page's body - on main the
# version is always the last released one, so a bump that lands without its
# notes file fails here, in the release PR, before the tag exists rather than
# in the tagged workflow after a build has already run.
#
# A bump that updates some of the four files goes green and ships, and every
# reader who copies the docs/evolve.md snippet pins the previous release, or
# the release page ends up untagged.
#
# So this checks all four, and fails when a pin is missing as loudly as when
# one is wrong: a docs rewrite that drops a pin, or a release PR that forgets
# the notes file, must not read as agreement.
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
    echo "::error file=docs/evolve.md::docs/evolve.md pins $pin while the workspace Cargo.toml says $version. The version moves four files together: Cargo.toml, docs/evolve.md, fluid/package.json and release-notes/v<version>.md." >&2
    bad=1
  fi
done <<< "$pins"

fluid="$(sed -n 's/.*"version": "\(.*\)".*/\1/p' "$root/fluid/package.json" | head -1)"
if [ "$fluid" != "$version" ]; then
  echo "::error file=fluid/package.json::fluid/package.json says $fluid and the workspace Cargo.toml says $version. Fluid compares its own version against the server's and warns the reader when they differ, so the two are bumped together." >&2
  bad=1
fi

# release-notes/v<version>.md must exist, be non-empty and open with an H1
# that carries text: the tagged release workflow's own `notes` job checks the
# same three things before any build starts, and this is what catches a
# missing file earlier, in the release PR, while it is still a one-line fix.
notesfile="$root/release-notes/v$version.md"
if [ ! -s "$notesfile" ]; then
  echo "::error file=release-notes/v$version.md::release-notes/v$version.md is missing or empty. The workspace version on main is always the last released one, so this file is the body the release page for v$version shows; add it in this PR before the tag is pushed." >&2
  bad=1
elif ! head -1 "$notesfile" | grep -qE '^# .+'; then
  echo "::error file=release-notes/v$version.md::release-notes/v$version.md must open with an H1 (a line starting with '# ' followed by text), the release's motto." >&2
  bad=1
fi

[ "$bad" = "0" ] || exit 1
echo "version pins: $version (Cargo.toml, 3 pins in docs/evolve.md, fluid/package.json, release-notes/v$version.md)"
