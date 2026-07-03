# Crystalline

[![CI](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml/badge.svg)](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/github/license/jordiboehme/crystalline)](LICENSE)
[![Latest release](https://img.shields.io/github/v/release/jordiboehme/crystalline)](https://github.com/jordiboehme/crystalline/releases/latest)

An AI agent starts every session as a stranger: no memory of yesterday's decisions, no sense of which conventions the team already settled on, everything re-derived from scratch or re-explained by a person. Crystalline gives an agent a durable memory instead. It is onboarded with a routing prompt at session start, taught curated knowledge organized into Domains and captures what it learns while it works as Engrams - so it becomes a more useful and productive peer over time instead of a stranger every time.

Crystalline is a single Rust binary: a CLI for people, an MCP server for agents, and a local search index that sits on top of plain markdown files.

## Why Crystalline

Crystalline is the evolution of approaches that many teams have walked through in the same order. Giving an agent a single markdown file of instructions works, until it grows past what fits in context. Splitting it into a folder of markdown files works, until nobody can tell which file to read for a given task. Adding index files that point at folders and other files works, until maintaining the pointers becomes its own job and every lookup still means walking a tree by hand. Each step scales further than the last, and each one quietly breaks somewhere in the hundreds of files.

Once knowledge grows into the thousands or tens of thousands of units, reading and pointer-walking stop being viable at all. What is needed at that scale is what any large knowledge system needs: real indexes. Crystalline keeps the plain markdown files - they remain the source of truth, readable and diffable - and adds domain routing, full-text and semantic search, a knowledge graph and temporal filtering on top, so the ten-thousandth engram is exactly as findable as the tenth.

## How it works

- **Domains** are folders of knowledge. Each one carries a `MANIFEST.md` describing its scope and when an agent should route a task there.
- **Engrams** are the unit of knowledge: one markdown file with YAML frontmatter, holding prose, observations (`- [category] a captured fact or lesson`) and relations (`- rel_type [[Other Engram]]`) to other engrams.
- **MANIFEST routing** lets an agent (or a person) figure out which domain owns a task without reading every file: `crystalline prompt system` turns each domain's `## When to Use` bullets into a compact session-start briefing.
- **Files are truth.** Engrams on disk are the only durable state. Nothing is ever stored only in the database.
- **The index is disposable.** Crystalline maintains a local embedded database for fast text, tag, temporal and semantic search, but it is fully derived from the markdown files and rebuilt on demand with `crystalline reindex --full`. Corruption or a schema change is never a data-loss event.

## Install

macOS, via [Homebrew](https://brew.sh):

```sh
brew install jordiboehme/tap/crystalline
```

Linux, via `.deb` (Debian, Ubuntu and derivatives, amd64 or arm64):

```sh
version=$(curl -fsSL https://api.github.com/repos/jordiboehme/crystalline/releases/latest | grep -m1 '"tag_name"' | cut -d '"' -f4)
arch=amd64   # or arm64
curl -fsSLO "https://github.com/jordiboehme/crystalline/releases/download/${version}/crystalline_${version#v}_${arch}.deb"
sudo dpkg -i "crystalline_${version#v}_${arch}.deb"
# or: sudo apt install "./crystalline_${version#v}_${arch}.deb"
crystalline --version
```

Anywhere else, download a prebuilt binary from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest). Four platforms are published:

| Platform | Archive |
|---|---|
| macOS (Apple Silicon) | `macos-arm64` |
| Linux x86_64 (statically linked) | `linux-amd64` |
| Linux arm64 (statically linked) | `linux-arm64` |
| Windows x86_64 | `windows-amd64` |

Each archive is named `crystalline-<version>-<platform>.tar.gz` (`.zip` on Windows) and contains the `crystalline` binary alongside `LICENSE` and `README.md`. A `SHA256SUMS` file is attached to every release for verification.

Shell one-liner (macOS/Linux, adjust `platform` to match your platform):

```sh
version=$(curl -fsSL https://api.github.com/repos/jordiboehme/crystalline/releases/latest | grep -m1 '"tag_name"' | cut -d '"' -f4)
platform=macos-arm64
curl -fsSL "https://github.com/jordiboehme/crystalline/releases/download/${version}/crystalline-${version}-${platform}.tar.gz" \
  | tar xz -C /tmp
sudo mv "/tmp/crystalline-${version}-${platform}/crystalline" /usr/local/bin/crystalline
crystalline --version
```

### Build from source

```sh
git clone https://github.com/jordiboehme/crystalline.git
cd crystalline
cargo build --release
```

The resulting binary is at `target/release/crystalline`.

## Quickstart

This runs verbatim, start to finish, on a clean machine.

```sh
# 1. Create a domain: a folder of knowledge with a MANIFEST.md at its root.
#    domain add indexes whatever is already there (the manifest, for now)
#    right away, no separate sync step needed.
mkdir -p ~/knowledge/engineering
crystalline domain init ~/knowledge/engineering --name engineering
crystalline domain add engineering ~/knowledge/engineering

# 2. Capture an engram: a unit of knowledge, with an observation bullet.
crystalline write engineering "Retry queue gotcha" \
  --content "- [gotcha] The retry queue drops jobs older than 24h #payments" \
  --tags gotcha,payments

# 3. Search it back (plain text, since no embeddings exist yet).
crystalline search "retry queue"

# 4. Fetch the local embedding model once, then re-sync with embeddings.
crystalline model download
crystalline sync --embed

# 5. Search again: hybrid text-plus-semantic ranking now finds the engram
#    from a differently worded description of the same problem.
crystalline search "why does the payments queue lose jobs"

# 6. See what got indexed.
crystalline status
```

Engrams written through Crystalline are indexed immediately; `crystalline sync` only picks up files created outside it (an editor, a `git pull`) when no daemon is watching them.

Edit `~/knowledge/engineering/MANIFEST.md`'s `## Scope` and `## When to Use` sections so routing and the session prompt describe the domain accurately - that file is what `crystalline prompt system` and an agent's routing decisions read.

## Connect your agent

Crystalline runs as an MCP server over stdio. Any MCP-capable harness works; add this to its MCP server configuration (this is the shape Claude Code's `.mcp.json` uses):

```json
{
  "mcpServers": {
    "crystalline": {
      "type": "stdio",
      "command": "crystalline",
      "args": ["mcp"]
    }
  }
}
```

The first agent to connect starts a background daemon that loads the embedding model once and watches every registered domain for changes; every later connection - other agents, other terminals, other harnesses - attaches to that same daemon instead of starting a second copy. One shared instance, one loaded model, one consistent view of the index, no matter how many agents are talking to it at once.

## Run in a container

Crystalline publishes a multi-arch OCI image (`linux/amd64` and `linux/arm64`) to GHCR on every release, for Linux server deployments. macOS and Windows have no OCI container runtime worth targeting here, so those platforms run the native binary from Install above; the container covers the Linux server case.

Two image variants ship under the same name, tag-selected:

| Tag | Size | Embedding model | Best for |
|---|---|---|---|
| `latest` (or a pinned `vX.Y.Z`) | ~15 MB | Downloads in the background on first daemon start (needs egress to huggingface.co once) | The common case: a host with normal internet access, where a short model download on first start is fine |
| `with-model` (or a pinned `vX.Y.Z-with-model`) | ~145 MB | Baked into the image, no download | Air-gapped or otherwise offline hosts, or anywhere semantic search must work from the very first `search` call with no warm-up delay |

Pick `with-model` whenever the host has no outbound network access or the first-start download delay is unwanted; pick the slim `latest` otherwise, since it is the smaller image to pull and update.

```sh
docker pull ghcr.io/jordiboehme/crystalline:latest
# or: docker pull ghcr.io/jordiboehme/crystalline:with-model

docker run -d \
  --name crystalline \
  -p 7411:7411 \
  -v "$(pwd)/knowledge:/knowledge" \
  -v crystalline-data:/data \
  ghcr.io/jordiboehme/crystalline:latest
```

What persists where:

- `./knowledge` (bind mount) holds the engrams themselves, one subfolder per domain - this is the only data that matters, and it is exactly the same markdown-plus-frontmatter files the native binary reads.
- `crystalline-data` (named volume, mounted at `/data`) holds the rebuildable search index and the embedding model cache. Losing it costs a `crystalline reindex --full` and a model re-download (skipped entirely on `with-model`, since its model lives outside `/data` and is never affected by the volume), never data.

The `with-model` variant sets `CRYSTALLINE_MODELS_DIR` (also settable directly, on any install, to relocate the model cache anywhere else) to a path outside `/data` so the baked model is never shadowed by the `/data` volume mount. The bundled model is [BAAI/bge-small-en-v1.5](https://huggingface.co/BAAI/bge-small-en-v1.5), MIT licensed.

Two sample Compose files ship under [`examples/docker/`](examples/docker/):

- **`compose.yaml`** - the single-container setup above, plus a commented one-shot `domain init` / `domain add` recipe for bootstrapping a fresh domain (`domain add` indexes it immediately, routed to the running daemon over the shared `/data` volume).
- **`compose.git-sync.yaml`** - a scale-deployment variant that adds a sidecar keeping the knowledge folder synced from a git remote every 60 seconds, mounted read-only into Crystalline. This is the pattern for a team that manages engrams as a reviewed git repository rather than writing into the container directly.

Agents connect to the containerized daemon over its HTTP MCP endpoint, `http://localhost:7411` from the host (the image's default command is `serve --http 0.0.0.0:7411`, since a container has to bind every interface to be reachable at all - binding `127.0.0.1` inside a container is only reachable from inside that same container). The stdio `crystalline mcp` transport from Connect your agent above is for local, non-containerized processes; point a harness at the HTTP endpoint instead when Crystalline runs in a container.

## Session onboarding

Run `crystalline prompt system` at the start of a session and feed its output to the agent as context. It reads every registered domain's `MANIFEST.md` and renders a compact routing block: one line per domain summarizing when to use it, plus the behavior rules (narrow question -> search that domain; broad question -> sweep all of them; writes always name a domain explicitly). The output names the exact crystalline MCP tools each rule refers to (`search_engrams`, `write_engram` and the rest), so an agent with several MCP servers connected knows exactly which tool on which server to call. `prompt` takes a subcommand naming the kind of prompt to generate; `system` is the only kind today.

```sh
crystalline prompt system --workspace .
```

Wire it into a harness with a generic recipe: run `crystalline prompt system` at session start and inject its stdout as context before the agent does anything else. In Claude Code, that is a `SessionStart` hook in `settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup",
        "hooks": [
          { "type": "command", "command": "crystalline prompt system" }
        ]
      }
    ]
  }
}
```

Any harness with an equivalent session-start hook can run the same command the same way.

## Teach and learn

The MCP server exposes 12 tools; capturing knowledge as a byproduct of work is the core loop:

- **`write_engram`** - capture a new engram. `domain` is always required (there is no default domain for writes, so an agent never writes into the wrong place). `permalink`, `status` and `recorded_at` are filled in for you.
- **`search_engrams`** - search before writing, and search to recall what is already known. Defaults to hybrid text-plus-semantic ranking across every domain; pass `domains` to narrow it, or filter by `type`, `tags`, `status` or arbitrary `metadata_filters` with no query text at all.
- **`edit_engram`** - refine an engram in place (`append`, `prepend`, `find_replace`, `replace_section`, `insert_before_section`, `insert_after_section`) instead of creating a duplicate for the same topic.
- **`build_context`** - given a `crystalline://domain/permalink` anchor, follow its relations and links (across domains too) to assemble the neighbourhood around a task before diving in.

Observations are the atomic unit of an engram's body: top-level bullets like `- [decision] we chose Postgres for the write path #database`. Categories are free text; useful ones include `decision`, `fact`, `pattern`, `gotcha`, `convention`, `lesson`, `risk` and `idea`. Relations connect engrams: `- depends_on [[Other Engram]]`, or `- "relates to" [[Other Engram]]` for a multi-word relation type.

Temporal fields are plain and easy to get wrong by overthinking them: an absent `valid_from` means the engram has always been valid, an absent `valid_to` means it is valid forever. Never write a sentinel far-future date - just leave the field out. Set them only when a fact is genuinely time-bounded (a policy that changes on a known date, a temporary workaround). `status` and `type` have recommended value sets stated in the tool descriptions themselves (status: `current`, `draft`, `idea`, `deprecated`, `superseded`, and so on; type: `engram`, `guide`, `decision`, `architecture`, `runbook`, `reference`) - they exist so an agent can tell an idea apart from current fact, and they are guidance, never a global enum a write is rejected for.

The CLI mirrors the mutating and read tools directly for scripting and quick edits outside an agent session: `crystalline write`, `read`, `edit`, `move`, `delete`, `search`, `context` and `recent` take the same parameters as their MCP counterparts.

## Keep knowledge honest

`crystalline verify` statically checks one or more domains against the full rule catalog - malformed frontmatter, broken links, missing MANIFEST sections, schema drift - with no database, service or network connection involved. Run it in CI with the bundled GitHub Action:

```yaml
- uses: jordiboehme/crystalline@v1
  with:
    paths: knowledge/
    strict: 'false'
```

The action downloads a pinned release binary (checksum-verified), runs `crystalline verify`, annotates the run and, on a pull request, posts a single summary comment kept up to date in place.

Two more commands keep a knowledge base trustworthy:

- **`crystalline import <src> --domain <name>`** brings an existing markdown-plus-frontmatter knowledge base under Crystalline: normalizes legacy `type` values, backfills `status` and temporal metadata, drops sentinel far-future dates in favor of leaving the field open-ended, and adds a missing `timestamp` - all as a pure file transformation, with `--dry-run` to preview first.
- **`crystalline doctor`** diagnoses the index, registered domains and service state (orphan index rows, encoding issues, stale service locks) and repairs what it safely can with `--fix`.

## Skills

The `skills/` folder ships three harness-agnostic agent skills that teach an agent how to use Crystalline well:

- **`crystalline-routing`** - which domain(s) to search for a task, when to sweep every domain instead, temporal filtering for "what is true now", and when to fall back to reading a MANIFEST directly.
- **`crystalline-capture`** - when captured knowledge is worth writing down, searching before writing to avoid duplicates, editing an existing engram instead of forking the topic, and the observation-category and temporal-field conventions that keep engrams useful later.
- **`crystalline-schema`** - authoring a Picoschema schema engram for a domain that wants structure, inferring one from what is already captured, and validating conformance.

Each is a plain folder with a `SKILL.md`; install by copying the folder into wherever your harness looks for skills. For Claude Code, that is `.claude/skills/` in a project or `~/.claude/skills/` globally:

```sh
cp -r skills/crystalline-routing skills/crystalline-capture skills/crystalline-schema ~/.claude/skills/
```

Other harnesses that support a similar skill or instruction-file convention can point at the same folders directly; the content only assumes the 12 MCP tools above, never a specific harness.

## Architecture

```
crystalline-core     format layer: parser, emitter, Picoschema, verify, prompt
       |              (no async runtime, no database, no ML - stays static)
       v
crystalline-index    Store trait, embedded database, sync engine, search, embeddings
       |
       v
crystalline-service  single-instance daemon, MCP tool router, control protocol
       |
       v
crystalline (cli)    the one user-facing binary
```

Exactly one process ever holds the database open: the first `crystalline mcp` or `crystalline serve` takes an advisory lock and becomes the daemon; every later CLI command or MCP connection attaches to it over a local socket, or opens the database directly for a brief operation when no daemon is running.

## Roadmap

- A PostgreSQL `Store` implementation for shared, multi-user deployments (the storage layer is already trait-based for this).
- Versioning and collaboration on a knowledge base through git.
- Authentication for the optional HTTP transport, which is unauthenticated today regardless of bind address. That is fine on the `127.0.0.1` default; the container image binds `0.0.0.0` so agents on the host can reach it, so treat the network boundary around the container (a private network, a reverse proxy, firewall rules) as the access control until this ships.

## License

GNU Affero General Public License v3.0 - see [LICENSE](LICENSE).
