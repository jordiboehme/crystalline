#!/usr/bin/env bash
# Style lint: rejects em dashes and en dashes in tracked markdown, Rust,
# JavaScript, shell and installer sources, keeping prose and CLI output in
# the plain-hyphen house style. It also refuses a "(s)" plural inside a Rust
# string literal under crates/*/src (log lines excepted).
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
# Also the JavaScript (the Claude Desktop launcher in packaging/mcpb/shim
# and its fixtures, whose text a user reads), the shell scripts, the WiX
# source of the MSI and the XML fixtures (the daemon task golden).
#
# Excluded: build output; vendored third-party files, which must stay
# byte-identical to their upstream (see evals/skill-training/vendor/README.md);
# the API types in fluid/src/api/gen, generated from the OpenAPI snapshot by
# `pnpm generate:api` and never hand-written; and fluid/pnpm-lock.yaml, which
# is generated too and is the one lockfile the patterns below can match.
files=$(git ls-files -- '*.md' '*.rs' '*.ts' '*.tsx' '*.css' '*.html' \
    '*Dockerfile' '*.dockerignore' '*.yml' '*.yaml' '*.conf' '*.template' \
    '*.js' '*.sh' '*.wxs' '*.xml' \
    | grep -v '/target/' \
    | grep -v '^target/' \
    | grep -v '/node_modules/' \
    | grep -v '^evals/skill-training/vendor/' \
    | grep -v '^fluid/src/api/gen/' \
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

# A "(s)" plural in a string a person reads: crystalline_core::text::plural
# says "1 file" and "3 files" instead. A string inside a tracing:: macro call
# is a log line and is left as it is. The check walks the Rust lexically, so
# "(s)" in a comment or in code such as `Some(s)` never counts, and a log
# call whose string sits on a later line than `tracing::` is still a log call.
rust_src=$(git ls-files -- 'crates/*/src/*.rs' | grep -v '/target/' || true)
if [ -n "$rust_src" ]; then
    # shellcheck disable=SC2086
    if ! perl - $rust_src <<'PERL'
use strict;
use warnings;

my $hits = 0;
for my $file (@ARGV) {
    open my $fh, '<', $file or die "$file: $!";
    my $src = do { local $/; <$fh> };
    close $fh;
    my ($pos, $boundary, $len) = (0, 0, length $src);
    while ($pos < $len) {
        pos($src) = $pos;
        if ($src =~ /\G\/\/[^\n]*/gc) {
            $pos = pos($src);
        } elsif ($src =~ /\G\/\*/gc) {
            my $depth = 1;
            while ($depth > 0 && $src =~ /\G(?:.*?)(\/\*|\*\/)/gcs) {
                $depth += $1 eq '/*' ? 1 : -1;
            }
            $pos = pos($src) // $len;
        } elsif ($src =~ /\G(?<![A-Za-z0-9_])b?r(#*)"/gc) {
            my $close = '"' . $1;
            my $end = index($src, $close, pos($src));
            $end = $len if $end < 0;
            check($file, \$src, $pos, $end, $boundary);
            $pos = $end + length $close;
        } elsif ($src =~ /\G(?<![A-Za-z0-9_])b?"(?:\\.|[^"\\])*"/gcs) {
            check($file, \$src, $pos, pos($src), $boundary);
            $pos = pos($src);
        } elsif ($src =~ /\G'(?:\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'/gc) {
            $pos = pos($src);
        } else {
            my $c = substr($src, $pos, 1);
            $boundary = $pos if $c eq ';' || $c eq '{' || $c eq '}';
            $pos++;
        }
    }
}
exit($hits ? 1 : 0);

sub check {
    my ($file, $src, $start, $end, $boundary) = @_;
    my $lit = substr($$src, $start, $end - $start);
    my $at = index($lit, '(s)');
    return if $at < 0;
    return if substr($$src, $boundary, $start - $boundary) =~ /tracing::/;
    my $line = 1 + (substr($$src, 0, $start + $at) =~ tr/\n//);
    print "style-lint: \"(s)\" plural in a string (use crystalline_core::text::plural): $file:$line\n";
    $hits++;
}
PERL
    then
        fail=1
    fi
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "style-lint: OK"
