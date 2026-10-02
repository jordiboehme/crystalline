#!/usr/bin/env bash
#
# Decides what a dev-channel run does for one version, so the dev channel only
# ever moves forward and a failed run can be finished by a re-run.
#
# CI runs on main can finish out of order (ci.yml has a docs-only fast path and
# no concurrency group), and an old CI run can be re-run. Either would publish
# a lower commit count and point the formula back at it. So:
#
# - build=true only when no release for this version exists yet and its commit
#   count N is strictly higher than every published dev pre-release's.
# - formula=true when the release exists or is about to be built, and the tap
#   formula names a lower N (or does not exist). That is what lets "Re-run all
#   jobs" finish a run whose publish worked but whose tap push failed: the
#   release exists, so nothing is rebuilt, but the formula is still behind.
#
# Drafts do not count as published. A cancelled `gh release create` can leave
# a draft behind (gh uploads the assets to a draft and publishes it last);
# dev.yml's publish job deletes a leftover draft of the same tag before it
# creates the release, so a draft never blocks a version.
#
# - A commit the stable release builds (a release PR's merge commit, named by
#   the optional <release-tag>, which dev.yml's release check sets) builds
#   nothing and leaves the formula alone when it would otherwise be a new
#   build: release.yml builds the same commit, and the next commit on main
#   builds the dev channel as usual. A release that already exists is still
#   finished as above.
#
#   bash scripts/dev-plan.sh <version> <releases.json> <formula.rb> [<release-tag>]
#
# <releases.json> is `gh release list --json tagName,isPrerelease,isDraft`.
# <formula.rb> is the tap's current Formula/crystalline-dev.rb; a missing or
# empty file means there is none yet. <release-tag> is empty or missing for
# every other commit. Prints build=... and formula=... for $GITHUB_OUTPUT, and
# the reasoning on stderr.
set -euo pipefail

version="${1:-}"
releases="${2:-}"
formula="${3:-}"
release_tag="${4:-}"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+-dev\.([0-9]+)$ ]]; then
  echo "dev-plan: '$version' is not a dev version like 0.22.0-dev.4817" >&2
  exit 1
fi
n="${BASH_REMATCH[1]}"
if [ ! -f "$releases" ]; then
  echo "dev-plan: release list '$releases' not found" >&2
  exit 1
fi

# The commit counts of the published dev pre-releases, one per line.
published="$(jq -r '
  .[]
  | select(.isPrerelease and (.isDraft | not))
  | .tagName
  | capture("^dev-[0-9]+\\.[0-9]+\\.[0-9]+-dev\\.(?<n>[0-9]+)$")?
  | .n' "$releases")"

max=0
while IFS= read -r p; do
  [ -n "$p" ] || continue
  if [ "$p" -gt "$max" ]; then
    max="$p"
  fi
done <<< "$published"

exists=false
if jq -e --arg tag "dev-$version" \
  'any(.[]; .tagName == $tag and .isPrerelease and (.isDraft | not))' "$releases" > /dev/null; then
  exists=true
fi

formula_n=0
if [ -n "$formula" ] && [ -s "$formula" ]; then
  current="$(sed -n 's/^  version "\(.*\)"$/\1/p' "$formula" | head -1)"
  if [[ "$current" =~ -dev\.([0-9]+)$ ]]; then
    formula_n="${BASH_REMATCH[1]}"
  fi
fi

echo "dev-plan: this build is N=$n, the newest published dev release N=$max, the formula N=$formula_n, released: $exists" >&2

if [ "$exists" = "true" ]; then
  build=false
  if [ "$formula_n" -lt "$n" ]; then
    formula_update=true
    echo "dev-plan: released already, but the formula is behind: update the formula only" >&2
  else
    formula_update=false
    echo "dev-plan: released already and the formula is not behind: nothing to do" >&2
  fi
elif [ "$n" -gt "$max" ] && [ -n "$release_tag" ]; then
  build=false
  formula_update=false
  echo "dev-plan: newer than every published dev release, but this is the commit the stable release $release_tag builds: nothing to do" >&2
elif [ "$n" -gt "$max" ]; then
  build=true
  formula_update=true
  echo "dev-plan: newer than every published dev release: build and publish" >&2
else
  build=false
  formula_update=false
  echo "dev-plan: not newer than N=$max, which is already published: a stale run ships nothing" >&2
fi

echo "build=$build"
echo "formula=$formula_update"
