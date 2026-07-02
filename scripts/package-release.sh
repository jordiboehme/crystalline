#!/usr/bin/env bash
# Package a local release archive for the current (or a given) target,
# mirroring what .github/workflows/release.yml builds per platform. Useful
# for producing a one-off artifact by hand or sanity-checking packaging
# before tagging a release. The workflow itself does not call this script;
# it stays self-contained so CI never depends on a local dev tool.
#
# Usage: scripts/package-release.sh [target]
# The target defaults to the current machine's host triple. The version is
# read from the workspace Cargo.toml and prefixed with 'v', matching the
# release tag format.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

target="${1:-$(rustc -vV | awk '/^host:/ { print $2 }')}"
version="v$(awk -F'"' '/^version = /{ print $2; exit }' Cargo.toml)"

echo "Building crystalline ${version} for ${target}..."
cargo build --release --target "$target" -p crystalline

bin="target/$target/release/crystalline"
ext="tar.gz"
if [[ "$target" == *windows* ]]; then
    bin="target/$target/release/crystalline.exe"
    ext="zip"
fi

if [[ ! -f "$bin" ]]; then
    echo "package-release: built binary not found at $bin" >&2
    exit 1
fi

name="crystalline-${version}-${target}"
stage="dist/$name"
rm -rf "$stage"
mkdir -p "$stage"
cp "$bin" "$stage/"
cp LICENSE "$stage/"
cp README.md "$stage/"

archive="dist/$name.$ext"
rm -f "$archive"
if [[ "$ext" == "zip" ]]; then
    (cd dist && zip -qr "$name.zip" "$name")
else
    (cd dist && tar czf "$name.tar.gz" "$name")
fi

checksum_file="$archive.sha256"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$archive" > "$checksum_file"
else
    shasum -a 256 "$archive" > "$checksum_file"
fi

echo "Packaged $archive"
cat "$checksum_file"
