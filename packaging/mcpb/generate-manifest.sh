#!/usr/bin/env bash
# Generates a manifest.json for one platform of the Crystalline MCP Bundle
# (.mcpb) and stages the bundle icon next to it. The release workflow calls
# this once per platform, then packs the staged directory with the
# `@anthropic-ai/mcpb` CLI.
#
# Usage: generate-manifest.sh <platform> <version> <outdir>
#   platform  one of macos-arm64, macos-amd64, windows-amd64, windows-arm64,
#             linux-amd64, linux-arm64
#   version   release version without a leading v, e.g. 0.2.0
#   outdir    directory manifest.json and icon.png are written into
#             (created if missing)
set -euo pipefail

usage() {
    echo "usage: $(basename "$0") <platform> <version> <outdir>" >&2
    echo "  platform: macos-arm64 | macos-amd64 | windows-amd64 | windows-arm64 | linux-amd64 | linux-arm64" >&2
    exit 1
}

if [ "$#" -ne 3 ]; then
    usage
fi

platform="$1"
version="$2"
outdir="$3"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

entry_point="server/crystalline"
# Literal placeholder for the mcpb host to expand at install time, not a
# shell variable: single-quoted on purpose.
# shellcheck disable=SC2016
command_path='${__dirname}/server/crystalline'
os_platform=""

case "$platform" in
    macos-arm64 | macos-amd64)
        os_platform="darwin"
        ;;
    windows-amd64 | windows-arm64)
        entry_point="server/crystalline.exe"
        # shellcheck disable=SC2016
        command_path='${__dirname}/server/crystalline.exe'
        os_platform="win32"
        ;;
    linux-amd64 | linux-arm64)
        os_platform="linux"
        ;;
    *)
        echo "error: unknown platform '$platform'" >&2
        usage
        ;;
esac

# Author identity: prefer the workspace Cargo.toml's [workspace.package]
# authors field (a TOML array of "Name <email>" strings, first entry wins),
# fall back to the maintainer identity when the field is absent.
author_name="Jordi Boehme"
author_email="jordi@boehme-lopez.de"
cargo_toml="$repo_root/Cargo.toml"

if [ -f "$cargo_toml" ]; then
    authors_line=$(grep -E '^[[:space:]]*authors[[:space:]]*=' "$cargo_toml" | head -n1 || true)
    if [ -n "$authors_line" ]; then
        first_author=$(printf '%s' "$authors_line" | sed -E 's/^[^=]*=[[:space:]]*\[[[:space:]]*"([^"]*)".*/\1/')
        if [ -n "$first_author" ] && [ "$first_author" != "$authors_line" ]; then
            case "$first_author" in
                *"<"*">"*)
                    author_name=$(printf '%s' "$first_author" | sed -E 's/[[:space:]]*<.*$//')
                    author_email=$(printf '%s' "$first_author" | sed -E 's/^[^<]*<([^>]*)>.*/\1/')
                    ;;
                *)
                    author_name="$first_author"
                    ;;
            esac
        fi
    fi
fi

mkdir -p "$outdir"
manifest_path="$outdir/manifest.json"

cat >"$manifest_path" <<JSON
{
  "manifest_version": "0.3",
  "name": "crystalline",
  "display_name": "Crystalline",
  "version": "$version",
  "description": "Durable memory for AI agents: teach knowledge in Domains, capture learnings as Engrams.",
  "long_description": "Crystalline gives an AI agent durable memory across sessions instead of starting from zero each time. The moment it connects, the server's own instructions carry a live routing index, so onboarding is automatic; from there the agent is taught information through curated Domains and captures what it learns and experiences as Engrams: markdown files with structured frontmatter that stay readable and editable outside of any agent.\n\nOver time an engram collection becomes a working memory the agent can search, browse and build context from before starting new work, turning it into a more useful peer with each session it runs. It starts with no domains: the agent creates one whenever it needs somewhere to capture knowledge, with the add_domain tool, as a folder of markdown files under your Documents/Crystalline folder, a database-backed domain or a GitHub team domain. A companion skill zip (crystalline-claude-desktop-skill on each release) teaches Claude capture and collaboration best practices; see the README's Skills section.",
  "author": {
    "name": "$author_name",
    "email": "$author_email"
  },
  "repository": {
    "type": "git",
    "url": "https://github.com/jordiboehme/crystalline.git"
  },
  "homepage": "https://github.com/jordiboehme/crystalline",
  "support": "https://github.com/jordiboehme/crystalline/issues",
  "icon": "icon.png",
  "license": "AGPL-3.0-or-later",
  "keywords": ["knowledge", "memory", "agent", "mcp", "markdown"],
  "server": {
    "type": "binary",
    "entry_point": "$entry_point",
    "mcp_config": {
      "command": "$command_path",
      "args": ["mcp"],
      "env": {}
    }
  },
  "compatibility": {
    "claude_desktop": ">=0.10.0",
    "platforms": ["$os_platform"]
  },
  "tools": [
    {
      "name": "write_engram",
      "description": "Capture a new engram, a unit of knowledge or experience, into a domain by writing its markdown file and indexing it."
    },
    {
      "name": "edit_engram",
      "description": "Refine an existing engram in place as understanding evolves, by section or with find and replace."
    },
    {
      "name": "move_engram",
      "description": "Re-home an engram to a new path or domain, rewriting inbound links so nothing dangles."
    },
    {
      "name": "delete_engram",
      "description": "Remove an engram when its knowledge is retired, deleting the file and its index rows."
    },
    {
      "name": "read_engram",
      "description": "Read an engram's full markdown and resolved frontmatter to learn what is already known."
    },
    {
      "name": "search_engrams",
      "description": "Search across domains with hybrid lexical and semantic ranking to recall relevant knowledge."
    },
    {
      "name": "build_context",
      "description": "Assemble the neighbourhood around an anchor engram by following its relations and links."
    },
    {
      "name": "recent_activity",
      "description": "Review what has been captured recently across domains to catch up on new knowledge."
    },
    {
      "name": "list_domains",
      "description": "List the registered domains with their engram counts to see what the agent has been taught."
    },
    {
      "name": "browse_domain",
      "description": "Browse a domain's engrams by folder to explore how its knowledge is organized."
    },
    {
      "name": "validate_engrams",
      "description": "Check a domain's engrams against its schema engrams to keep captured knowledge well-formed."
    },
    {
      "name": "infer_schema",
      "description": "Suggest a Picoschema for a type by generalizing over engrams already captured in a domain."
    },
    {
      "name": "configure",
      "description": "View and adjust Crystalline's settings, like connecting a GitHub account for team collaboration."
    },
    {
      "name": "add_domain",
      "description": "Create or connect a domain to capture engrams in: a local folder of markdown files, a database-backed virtual domain or a GitHub team domain."
    }
  ],
  "tools_generated": false
}
JSON

if ! jq . "$manifest_path" >/dev/null; then
    echo "error: generated manifest is not valid JSON: $manifest_path" >&2
    exit 1
fi

cp "$repo_root/assets/crystalline.png" "$outdir/icon.png"

echo "wrote $manifest_path"
