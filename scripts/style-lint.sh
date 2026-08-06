#!/usr/bin/env bash
# Style lint: rejects em dashes and en dashes in tracked markdown and Rust
# source, keeping prose and CLI output in the plain-hyphen house style.
#
# The Oxford comma was banned here until 2026-08-04 and is now allowed, so
# that check is gone. Existing text was left as written rather than
# rewritten, so both list styles appear in the tree.
set -euo pipefail

# Paths from git ls-files are relative to the working directory; run from
# the repository root so the exclusion patterns below always match.
cd "$(git rev-parse --show-toplevel)"

fail=0

# Tracked markdown, Rust and Fluid front-end sources, plus the deployment
# files (Dockerfiles, compose and workflow YAML, nginx configuration and its
# templates), which carry as much prose in comments as anything else here.
#
# Excluded: build output; vendored third-party files, which must stay
# byte-identical to their upstream (see evals/skill-training/vendor/README.md);
# the API types fluid/src/api/types.ts, generated from the OpenAPI snapshot by
# `pnpm generate:api` and never hand-written; and fluid/pnpm-lock.yaml, which
# is generated too and is the one lockfile the patterns below can match.
files=$(git ls-files -- '*.md' '*.rs' '*.ts' '*.tsx' '*.css' '*.html' \
    '*Dockerfile' '*.dockerignore' '*.yml' '*.yaml' '*.conf' '*.template' \
    | grep -v '/target/' \
    | grep -v '^target/' \
    | grep -v '/node_modules/' \
    | grep -v '^evals/skill-training/vendor/' \
    | grep -v '^fluid/src/api/types\.ts$' \
    | grep -v '^fluid/pnpm-lock\.yaml$' || true)

if [ -z "$files" ]; then
    echo "style-lint: no tracked source files found"
    exit 0
fi

# UTF-8 byte sequences for em dash (U+2014) and en dash (U+2013), built
# with printf so this file itself never contains the raw characters.
em_dash=$(printf '\xe2\x80\x94')
en_dash=$(printf '\xe2\x80\x93')

for f in $files; do
    hits=$(LC_ALL=C grep -n -e "$em_dash" -e "$en_dash" "$f" 2>/dev/null || true)
    if [ -n "$hits" ]; then
        echo "$hits" | while IFS= read -r line; do
            echo "style-lint: em dash or en dash (use '-' instead): $f:$line"
        done
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "style-lint: OK"
