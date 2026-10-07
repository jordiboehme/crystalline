#!/usr/bin/env bash
# Generates the manifest.json of one Crystalline MCP Bundle (.mcpb) and
# stages what the bundle needs beside it. The release workflow calls this
# once per bundle, then packs the staged directory with the
# `@anthropic-ai/mcpb` CLI.
#
# Usage: generate-manifest.sh <bundle> <version> <outdir>
#   bundle    standard: the extension for macOS and Windows, a Node launcher
#             that runs the Crystalline Homebrew or the MSI installed; this
#             script stages server/shim.js itself.
#             desktop-only: macOS only, the crystalline binary inside (the
#             caller stages server/crystalline, a universal binary).
#   version   release version without a leading v, e.g. 0.24.0
#   outdir    directory manifest.json, icon.png and server/ are written into
#             (created if missing)
set -euo pipefail

usage() {
    echo "usage: $(basename "$0") <bundle> <version> <outdir>" >&2
    echo "  bundle: standard | desktop-only" >&2
    exit 1
}

if [ "$#" -ne 3 ]; then
    usage
fi

bundle="$1"
version="$2"
outdir="$3"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

display_name="Crystalline"
case "$bundle" in
    standard)
        server_type="node"
        entry_point="server/shim.js"
        mcp_command="node"
        # Literal placeholder for the mcpb host to expand at install time.
        # shellcheck disable=SC2016
        mcp_args='["${__dirname}/server/shim.js"]'
        env_fragment=""
        platforms_json='"darwin", "win32"'
        lead="This extension connects Claude Desktop to the Crystalline you installed with Homebrew (macOS) or the MSI (Windows); install that first."
        ;;
    desktop-only)
        server_type="binary"
        entry_point="server/crystalline"
        # shellcheck disable=SC2016
        mcp_command='${__dirname}/server/crystalline'
        mcp_args='["mcp"]'
        env_fragment=',
      "env": {
        "CRYSTALLINE_CHANNEL": "mcpb"
      }'
        platforms_json='"darwin"'
        display_name="Crystalline (Claude Desktop only)"
        lead="This macOS extension carries Crystalline inside, for a Mac without Homebrew. With Homebrew, take the standard extension instead."
        ;;
    *)
        echo "error: unknown bundle '$bundle'" >&2
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

# The text both bundles share after their own first sentence.
long_rest="In psychology, fluid intelligence reasons in the moment while crystallized intelligence is what learning has accumulated. A language model is all fluid; Crystalline gives an AI agent the crystalline half: knowledge that endures across sessions instead of starting from zero each time. The moment it connects, the server's own instructions carry a live routing index, so onboarding is automatic; from there the agent is taught information through curated Domains and captures what it learns and experiences as Engrams: markdown files with structured frontmatter that stay readable and editable outside of any agent.\n\nOver time an engram collection becomes a crystalline intelligence the agent can search, browse and build context from before starting new work, turning it into a more useful peer with each session it runs. It starts with no domains: the agent creates one whenever it needs somewhere to capture knowledge, with the add_domain tool, as a folder of markdown files under your Documents/Crystalline folder, a database-backed domain or a GitHub team domain. A companion skill zip (crystalline-claude-desktop-skill on each release) teaches Claude capture and collaboration best practices; see the README's Skills section."

mkdir -p "$outdir"
manifest_path="$outdir/manifest.json"

cat >"$manifest_path" <<JSON
{
  "manifest_version": "0.3",
  "name": "crystalline",
  "display_name": "$display_name",
  "version": "$version",
  "description": "Crystalline intelligence for AI agents: teach knowledge in Domains, capture learnings as Engrams.",
  "long_description": "$lead $long_rest",
  "author": {
    "name": "$author_name",
    "email": "$author_email"
  },
  "repository": {
    "type": "git",
    "url": "https://github.com/jordiboehme/crystalline.git"
  },
  "homepage": "https://github.com/jordiboehme/crystalline",
  "documentation": "https://github.com/jordiboehme/crystalline/tree/main/docs",
  "support": "https://github.com/jordiboehme/crystalline/issues",
  "icon": "icon.png",
  "license": "MPL-2.0",
  "privacy_policies": ["https://github.com/jordiboehme/crystalline#privacy-policy", "https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement", "https://huggingface.co/privacy"],
  "keywords": ["knowledge", "memory", "agent", "mcp", "markdown"],
  "server": {
    "type": "$server_type",
    "entry_point": "$entry_point",
    "mcp_config": {
      "command": "$mcp_command",
      "args": $mcp_args$env_fragment
    }
  },
  "compatibility": {
    "claude_desktop": ">=0.10.0",
    "platforms": [$platforms_json]
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
      "description": "Check a domain's engrams against its schema engrams to keep captured knowledge well-formed; pass drift to also report observation categories and relation types in use but undeclared, and declared but unused."
    },
    {
      "name": "infer_schema",
      "description": "Suggest a Picoschema for a type by generalizing over engrams already captured in a domain."
    },
    {
      "name": "vocabulary",
      "description": "List the tags, observation categories and relation types already in use, with counts, so existing terms are reused."
    },
    {
      "name": "evolve_engrams",
      "description": "Sweep a domain for the maintenance the knowledge needs and return a ranked read-only work queue: lifecycle debt, structural gaps and near-duplicates, each with its evidence and the exact next action."
    },
    {
      "name": "configure",
      "description": "View and adjust Crystalline's settings, like connecting a GitHub account for team collaboration."
    },
    {
      "name": "add_domain",
      "description": "Create or connect a domain to capture engrams in: a local folder of markdown files, a database-backed virtual domain or a GitHub team domain."
    },
    {
      "name": "skills",
      "description": "List the agent skills this server ships and read any skill's full playbook before its kind of task."
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
if [ "$bundle" = standard ]; then
    mkdir -p "$outdir/server"
    cp "$script_dir/shim/shim.js" "$outdir/server/shim.js"
fi

echo "wrote $manifest_path"
