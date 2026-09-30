#!/usr/bin/env bash
#
# Renders Formula/crystalline-dev.rb for the jordiboehme/homebrew-tap, the
# Homebrew dev channel, to stdout.
#
# It is the stable formula release.yml writes (Formula/crystalline.rb), with
# four differences: the class is CrystallineDev, the archives come from the
# dev-<version> pre-release, it conflicts with the stable formula because both
# install the same `crystalline` binary, and its caveats say the channel only
# moves forward. The stable formula is written by release.yml alone and this
# script never touches it.
#
# .github/workflows/dev.yml calls this after it published the pre-release.
#
#   bash scripts/render-dev-formula.sh <version> <SHA256SUMS>
#
# <version> is the dev version without a prefix (0.22.0-dev.4817), and
# <SHA256SUMS> the checksum file of that pre-release.
set -euo pipefail

version="${1:-}"
sums="${2:-}"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+-dev\.[0-9]+$ ]]; then
  echo "render-dev-formula: '$version' is not a dev version like 0.22.0-dev.4817" >&2
  exit 1
fi
if [ ! -s "$sums" ]; then
  echo "render-dev-formula: checksum file '$sums' is missing or empty" >&2
  exit 1
fi

sha_for() {
  local archive="crystalline-v${version}-$1.tar.gz"
  local sha
  sha="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$sums")"
  if ! [[ "$sha" =~ ^[0-9a-f]{64}$ ]]; then
    echo "render-dev-formula: no sha256 for $archive in $sums" >&2
    exit 1
  fi
  echo "$sha"
}

sha_macos_arm="$(sha_for macos-arm64)"
sha_macos_intel="$(sha_for macos-intel)"
sha_linux_amd64="$(sha_for linux-amd64)"
sha_linux_arm64="$(sha_for linux-arm64)"

cat << FORMULA
class CrystallineDev < Formula
  desc "Local-first knowledge management for humans and AI agents (dev builds)"
  homepage "https://github.com/jordiboehme/crystalline"
  license "AGPL-3.0-or-later"
  version "${version}"

  conflicts_with "crystalline", because: "both install the crystalline binary"

  on_macos do
    on_arm do
      url "https://github.com/jordiboehme/crystalline/releases/download/dev-#{version}/crystalline-v#{version}-macos-arm64.tar.gz"
      sha256 "${sha_macos_arm}"
    end

    on_intel do
      url "https://github.com/jordiboehme/crystalline/releases/download/dev-#{version}/crystalline-v#{version}-macos-intel.tar.gz"
      sha256 "${sha_macos_intel}"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/jordiboehme/crystalline/releases/download/dev-#{version}/crystalline-v#{version}-linux-amd64.tar.gz"
      sha256 "${sha_linux_amd64}"
    end

    on_arm do
      url "https://github.com/jordiboehme/crystalline/releases/download/dev-#{version}/crystalline-v#{version}-linux-arm64.tar.gz"
      sha256 "${sha_linux_arm64}"
    end
  end

  def install
    bin.install "crystalline"
  end

  def caveats
    <<~EOS
      This is a dev build from main. It may migrate your index, and going back to an older stable release is not supported.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/crystalline --version")
  end
end
FORMULA
