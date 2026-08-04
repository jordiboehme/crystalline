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

# Tracked markdown and Rust files, excluding build output and vendored
# third-party files, which must stay byte-identical to their upstream
# (see evals/skill-training/vendor/README.md).
files=$(git ls-files -- '*.md' '*.rs' | grep -v '/target/' | grep -v '^target/' | grep -v '^evals/skill-training/vendor/' || true)

if [ -z "$files" ]; then
    echo "style-lint: no tracked .md or .rs files found"
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
