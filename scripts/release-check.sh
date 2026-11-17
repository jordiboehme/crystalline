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
# - the tag exists and so does its GitHub release: the version did not
#   change since the last release, so there is nothing to release. One line,
#   release=false, exit 0.
# - the tag exists but has no GitHub release: exit 1, naming the stray tag
#   and how to remove it. A tag pushed by hand, or a release run that stopped
#   after it made the tag, would otherwise read as released, and that
#   version would never ship.
# - the tag is new and release-notes/v<version>.md is there, not empty and
#   opens with an H1: release=true, exit 0.
# - the tag is new and the notes file is missing or has no H1: exit 1. The
#   version moved without its notes, so the release pull request forgot them.
#   This fails loudly instead of skipping quietly, because a quiet skip would
#   leave a version on main that never ships.
#
# A tag lookup that fails (network, a remote that does not answer) also exits
# 1. It never reads as "no tag", which would start a release. A release
# lookup that fails exits 1 as well, and never reads as "no release".
#
# The release lookup goes through `gh api`, so it needs gh and a token
# (GH_TOKEN on GitHub Actions, `gh auth login` locally). The repository is
# $GITHUB_REPOSITORY on GitHub Actions, else $RELEASE_CHECK_REPO, else the
# owner/name of the remote's GitHub URL.
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

# The owner/name of the GitHub repository whose releases are looked up.
repo_slug() {
  if [ -n "${GITHUB_REPOSITORY:-}" ]; then
    printf '%s\n' "$GITHUB_REPOSITORY"
  elif [ -n "${RELEASE_CHECK_REPO:-}" ]; then
    printf '%s\n' "$RELEASE_CHECK_REPO"
  else
    local url
    url="$(git -C "$root" remote get-url "$remote")" || return 1
    url="${url%.git}"
    if [[ "$url" =~ github\.com[:/]([^/]+/[^/]+)$ ]]; then
      printf '%s\n' "${BASH_REMATCH[1]}"
    else
      echo "::error::the remote $remote ($url) is no GitHub repository; set RELEASE_CHECK_REPO to owner/name" >&2
      return 1
    fi
  fi
}

# Whether the GitHub release for a tag exists: prints yes or no. Returns 1
# on a lookup error, which never reads as "no release".
release_exists() {
  local tag="$1" repo err rc=0
  repo="$(repo_slug)" || return 1
  if ! command -v gh >/dev/null 2>&1; then
    echo "::error::gh is not installed, so the release for $tag cannot be looked up" >&2
    return 1
  fi
  err="$(gh api --silent "repos/$repo/releases/tags/$tag" 2>&1 >/dev/null)" || rc=$?
  if [ "$rc" -eq 0 ]; then
    echo yes
  elif grep -q 'HTTP 404' <<<"$err"; then
    echo no
  else
    echo "::error::could not look up the release $tag in $repo (gh api exit $rc: $err)" >&2
    return 1
  fi
}

# A tag that names this version with no GitHub release behind it: fail with
# what happened and how to remove it.
stray_tag() {
  local tag="$1"
  echo "::error::the tag $tag exists but there is no GitHub release for it. It was pushed by hand, or a release run stopped after it made the tag. While the tag is there no merge releases $version. If a release run failed, re-run its failed jobs; otherwise delete the tag with 'git push origin :refs/tags/$tag' (and 'git tag -d $tag' locally) and push to main again." >&2
  exit 1
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

where=""
if git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  where="here"
else
  commit="$(tag_commit "$tag")"
  if [ -n "$commit" ]; then
    where="on $remote (at $commit)"
  fi
fi
if [ -n "$where" ]; then
  released="$(release_exists "$tag")"
  [ "$released" = yes ] || stray_tag "$tag"
  echo "release: $tag is already tagged $where and released, nothing to release"
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
