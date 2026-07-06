#!/usr/bin/env bash
# Regenerate the fixture workspaces with the real crystalline binary.
set -euo pipefail
cd "$(dirname "$0")/.."
exec uv run python fixtures/generate.py "$@"
