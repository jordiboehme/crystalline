#!/usr/bin/env bash
#
# Decides whether a push to main releases, and looks up where a tag points.
#
# A release is the last pull request of a version: it bumps the version and
# adds release-notes/v<version>.md, and merging it releases. CI's `release`
# jobs on main run this after `ci-passed`:
#
#   bash scripts/release-check.sh
#
# reads the workspace version from Cargo.toml and asks whether the tag
# v<version> exists, here or on the remote:
#
# - the tag exists: the version did not change since the last release, so
#   there is nothing to release. One line, release=false, exit 0.
# - the tag is new and release-notes/v<version>.md is there, not empty and
#   opens with an H1: release=true, exit 0.
# - the tag is new and the notes file is missing or has no H1: exit 1. The
#   version moved without its notes, so the release pull request forgot them.
#   This fails loudly instead of skipping quietly, because a quiet skip would
#   leave a version on main that never ships.
#
# A tag lookup that fails (network, a remote that does not answer) also exits
# 1. It never reads as "no tag", which would start a release.
#
# On GitHub Actions the answer goes to $GITHUB_OUTPUT as `tag=v<version>` and
# `release=true|false`. Locally it is only printed.
#
#   bash scripts/release-check.sh tag-commit v1.2.3
#
# prints the commit the tag points to (peeled, so an annotated tag gives its
# commit), or nothing when the tag does not exist; exit 1 on a lookup error.
#
# The remote is `origin`, or $RELEASE_CHECK_REMOTE when set.
#
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
remote="${RELEASE_CHECK_REMOTE:-origin}"

# The commit a tag points to, from the remote, peeled. Empty when the remote
# has no such tag. Returns 1 on a lookup error.
tag_commit() {
  local tag="$1" out rc=0
  out="$(git -C "$root" ls-remote --exit-code --tags "$remote" \
    "refs/tags/$tag" "refs/tags/$tag^{}")" || rc=$?
  case "$rc" in
    0) ;;
    2) return 0 ;;
    *)
      echo "::error::could not look up the tag $tag on the remote $remote (git ls-remote exit $rc)" >&2
      return 1
      ;;
  esac
  # An annotated tag lists the tag object and then the peeled commit (^{});
  # a lightweight tag lists the commit alone.
  local peeled plain
  peeled="$(awk -v r="refs/tags/$tag^{}" '$2 == r {print $1}' <<<"$out")"
  plain="$(awk -v r="refs/tags/$tag" '$2 == r {print $1}' <<<"$out")"
  printf '%s\n' "${peeled:-$plain}"
}

output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf '%s=%s\n' "$1" "$2" >>"$GITHUB_OUTPUT"
  fi
}

if [ "${1:-}" = "tag-commit" ]; then
  [ -n "${2:-}" ] || { echo "usage: $0 tag-commit <tag>" >&2; exit 2; }
  tag_commit "$2"
  exit 0
fi
[ "$#" -eq 0 ] || { echo "usage: $0 [tag-commit <tag>]" >&2; exit 2; }

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -1)"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "::error file=Cargo.toml::no workspace version of the form X.Y.Z in Cargo.toml (found '$version')" >&2
  exit 1
fi
tag="v$version"
output tag "$tag"

if git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "release: $tag is already tagged here, nothing to release"
  output release false
  exit 0
fi
commit="$(tag_commit "$tag")"
if [ -n "$commit" ]; then
  echo "release: $tag is already tagged on $remote (at $commit), nothing to release"
  output release false
  exit 0
fi

notes="release-notes/$tag.md"
if [ ! -s "$root/$notes" ]; then
  echo "::error file=$notes::the version moved to $version but $notes is missing or empty. Add the notes on main to release $tag." >&2
  exit 1
fi
if ! head -1 "$root/$notes" | grep -qE '^# .+'; then
  echo "::error file=$notes::$notes must open with an H1 (a line starting with '# ' followed by text), the release's motto." >&2
  exit 1
fi

echo "release: $tag is new and $notes is in place, so this commit releases $tag"
output release true
