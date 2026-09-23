#!/usr/bin/env bash
#
# Applies release-notes/<tag>.md files to their published GitHub Releases.
# The file is the one source of truth for a release page's body, so this is
# the step that makes a correction to old notes a commit rather than a
# manual edit: land the fix on main and the page catches up.
#
# .github/workflows/release-notes.yml calls this with no --dry-run after a
# push to main touches release-notes/. Run it by hand with --dry-run first
# to see what a push would do without changing anything on GitHub.
#
#   scripts/sync-release-notes.sh [--dry-run] [file...]
#
# With no files, syncs every release-notes/v*.md (what a workflow_dispatch
# run does). Given files, syncs only those (what a push does, once the
# workflow has narrowed the diff to what actually changed). A file that no
# longer exists, from a delete in the same push, is silently skipped: this
# script only ever sees files that are still there.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

dry_run=0
files=()
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    *) files+=("$arg") ;;
  esac
done

if [ "${#files[@]}" -eq 0 ]; then
  shopt -s nullglob
  files=(release-notes/v*.md)
  shopt -u nullglob
fi

for file in "${files[@]}"; do
  [ -f "$file" ] || continue

  base="$(basename "$file")"
  tag="${base%.md}"

  # A release for the tag not existing yet is the expected case for a
  # release PR that merged its notes file before the tag was pushed, not an
  # error, so a failure here (release missing, or any other gh error) reads
  # the same way: there is nothing to sync yet.
  if ! body_remote="$(gh release view "$tag" --json body --jq .body 2>/dev/null)"; then
    echo "no release yet: $tag"
    continue
  fi

  # Command substitution strips every trailing newline from both sides, which
  # is exactly the "trailing newlines trimmed" comparison the file (always
  # ending in exactly one) and the release body (whatever gh's JSON round trip
  # produces) need before they can be compared for real content differences.
  body_local="$(cat "$file")"

  if [ "$body_remote" = "$body_local" ]; then
    echo "unchanged: $tag"
    continue
  fi

  if [ "$dry_run" = "1" ]; then
    echo "would update: $tag"
    continue
  fi

  gh release edit "$tag" --notes-file "$file"
  echo "updated: $tag"
done
