#!/usr/bin/env bash
#
# Prints the dev-channel version of a commit: <X>.<Y+1>.0-dev.<N>, where X.Y.Z
# is the workspace version in that commit's Cargo.toml and N is the number of
# commits reachable from it (`git rev-list --count`).
#
# The workspace version on main is always the last released one, so the next
# minor is what main is working towards. N counts the whole history, so it
# only ever grows on main and the version is monotonic across stable and patch
# releases: once a release PR bumps Cargo.toml to 0.21.0, dev moves on to
# 0.22.0-dev.<N>. Semantic versioning orders 0.22.0-dev.<N> below 0.22.0, which
# is what the daemon takeover compares (crates/service/src/instance.rs).
#
# .github/workflows/dev.yml calls this on the commit CI tested. It needs the
# full history (actions/checkout with fetch-depth: 0); a shallow clone would
# count only the commits it holds, so this refuses one.
#
#   bash scripts/dev-version.sh [commit]
#
# The commit defaults to HEAD.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
commit="${1:-HEAD}"

if [ "$(git -C "$root" rev-parse --is-shallow-repository)" = "true" ]; then
  echo "dev-version: this clone is shallow, so the commit count would be wrong. Fetch the full history (actions/checkout with fetch-depth: 0)." >&2
  exit 1
fi

version="$(git -C "$root" show "$commit:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)"
if ! [[ "$version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "dev-version: expected a plain X.Y.Z workspace version in Cargo.toml at $commit, found '$version'" >&2
  exit 1
fi
major="${BASH_REMATCH[1]}"
minor="${BASH_REMATCH[2]}"

count="$(git -C "$root" rev-list --count "$commit")"

echo "${major}.$((minor + 1)).0-dev.${count}"
