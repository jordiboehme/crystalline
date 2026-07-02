#!/usr/bin/env bash
# Style lint: rejects em dashes, en dashes and Oxford-comma-style lists
# ("a, b and c" is fine, "a, b, and c" is not) in tracked markdown and
# Rust source. Keeps prose and CLI output in the plain-hyphen, no
# trailing-comma-before-and house style.
set -euo pipefail

fail=0

# Tracked markdown and Rust files, excluding build output.
files=$(git ls-files -- '*.md' '*.rs' | grep -v '/target/' | grep -v '^target/' || true)

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

# Oxford-comma heuristic: "word, word, and " / "word, word, or ".
# Checked in markdown prose, and inside Rust string literals only (a
# quoted run containing the pattern) so normal Rust syntax is untouched.
oxford_pattern='[A-Za-z0-9_]+, [A-Za-z0-9_]+, (and|or) '
oxford_in_string_pattern='"[^"]*'"$oxford_pattern"'[^"]*"'

for f in $files; do
    case "$f" in
        *.md)
            hits=$(grep -nE "$oxford_pattern" "$f" 2>/dev/null || true)
            ;;
        *.rs)
            hits=$(grep -nE "$oxford_in_string_pattern" "$f" 2>/dev/null || true)
            ;;
        *)
            hits=""
            ;;
    esac
    if [ -n "$hits" ]; then
        echo "$hits" | while IFS= read -r line; do
            echo "style-lint: Oxford-comma style list (write 'a, b and c' instead): $f:$line"
        done
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "style-lint: OK"
