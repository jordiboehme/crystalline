```
                                   ·              *
                                 ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄
                                ▐░░░▒▒▒▒▓▓▓█▓▓▓▒▒▒▒░░░▌
                                 ▀█░░░▒▒▒▓▓█▓▓▒▒▒░░░█▀   ·
                                   ▀█░░▒▒▒▓█▓▒▒▒░░█▀
                            *        ▀█░▒▒▓█▓▒▒░█▀
                                       ▀█▒▒█▒▒█▀
                                         ▀███▀     ·
                                           ▀

 ██████╗██████╗ ██╗   ██╗███████╗████████╗ █████╗ ██╗     ██╗     ██╗███╗   ██╗███████╗
██╔════╝██╔══██╗╚██╗ ██╔╝██╔════╝╚══██╔══╝██╔══██╗██║     ██║     ██║████╗  ██║██╔════╝
██║     ██████╔╝ ╚████╔╝ ███████╗   ██║   ███████║██║     ██║     ██║██╔██╗ ██║█████╗
██║     ██╔══██╗  ╚██╔╝  ╚════██║   ██║   ██╔══██║██║     ██║     ██║██║╚██╗██║██╔══╝
╚██████╗██║  ██║   ██║   ███████║   ██║   ██║  ██║███████╗███████╗██║██║ ╚████║███████╗
 ╚═════╝╚═╝  ╚═╝   ╚═╝   ╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═══╝╚══════╝
```

[![CI](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml/badge.svg)](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/github/license/jordiboehme/crystalline)](LICENSE)
[![Latest release](https://img.shields.io/github/v/release/jordiboehme/crystalline)](https://github.com/jordiboehme/crystalline/releases/latest)

An AI agent starts every session as a stranger: no memory of yesterday's decisions, no sense of which conventions the team already settled on, everything re-derived from scratch via Markdown files, expensive exploration or re-explained by a person. Crystalline gives an agent a durable memory instead. It is onboarded with a routing prompt at session start, taught curated knowledge organized into Domains and captures what it learns while it works as Engrams - so it becomes a more useful and productive peer over time instead of a stranger every time.

Crystalline is a single Rust binary: a CLI for people, an MCP server for agents, and a local search index that sits on top of plain markdown files.

## Why Crystalline

Crystalline is the evolution of approaches that many teams have walked through in the same order. Giving an agent a single markdown file of instructions works, until it grows past what fits in context. Splitting it into a folder of markdown files works, until nobody can tell which file to read for a given task. Adding index files that point at folders and other files works, until maintaining the pointers becomes its own job and every lookup still means walking a tree by hand. Each step scales further than the last, and each one quietly breaks somewhere in the hundreds of files.

Once knowledge grows into the thousands or tens of thousands of units, reading and pointer-walking stop being viable at all. What is needed at that scale is what any large knowledge system needs: real indexes. Crystalline keeps the plain markdown files - they remain the source of truth, readable and diffable - and adds domain routing, full-text and semantic search, a knowledge graph and temporal filtering on top, so the ten-thousandth engram is exactly as findable as the tenth.

## How it works

- **Domains** are folders of knowledge. Each one carries a `MANIFEST.md` describing its scope and when an agent should route a task there.
- **Engrams** are the unit of knowledge: one markdown file with YAML frontmatter, holding prose, observations (`- [category] a captured fact or lesson`) and relations (`- rel_type [[Other Engram]]`) to other engrams.
- **Built on an open format.** The engram format extends [Google's Open Knowledge Format (OKF)](https://github.com/GoogleCloudPlatform/knowledge-catalog/tree/main/okf): plain markdown with YAML frontmatter where unknown keys are always preserved. Crystalline layers its routing, temporal and knowledge-graph conventions on top, so engrams stay readable by any OKF tooling and OKF documents drop into a domain with minimal ceremony.
- **Knowledge retires, it does not disappear.** When a fact stops holding, the old engram is superseded rather than overwritten: its `status` marks it as no longer current, `valid_from`/`valid_to` keep the past addressable by date ("what applied last June") and the lessons it taught carry forward as unbounded knowledge - the way a person still draws on a past job without mistaking it for the present.
- **MANIFEST routing** lets an agent (or a person) figure out which domain owns a task without reading every file: `crystalline prompt system` turns each domain's `## When to Use` bullets into a compact session-start briefing.
- **One truth per domain.** By default files are truth: engrams on disk are the durable state and nothing lives only in the database.* The search index is always derived and disposable, whichever side holds the truth.

  *Unless you ask for exactly that: a virtual domain keeps its engrams in the database instead of on disk, for deployments where a filesystem is baggage rather than a feature. The principle does not bend - it just lets you pick which side of it your domain lives on, and `crystalline domain export` hands the files back whenever you change your mind.
- **The index is disposable.** Crystalline maintains a database for fast text, tag, temporal and semantic search. For a file domain it is fully derived from the markdown files and rebuilt on demand with `crystalline reindex --full`; for a virtual domain the same tables hold the source of truth, so `reindex --full` rebuilds the file domains around it and never touches it. Corruption or a schema change is never a data-loss event.

## Get started

Install the binary, run one command to wire up your harness and give the agent a first domain to learn into. Claude Desktop skips the binary entirely - jump straight to [its subsection](#claude-desktop). Semantic search wants the local embedding model fetched once with `crystalline model download`; plain text search works before that.

### Install the binary

macOS, via [Homebrew](https://brew.sh):

```sh
brew install jordiboehme/tap/crystalline
```

Linux, via `.deb` package (Debian, Ubuntu and derivatives, amd64 or arm64):

```sh
version=$(curl -fsSL https://api.github.com/repos/jordiboehme/crystalline/releases/latest | grep -m1 '"tag_name"' | cut -d '"' -f4)
arch=amd64   # or arm64
curl -fsSLO "https://github.com/jordiboehme/crystalline/releases/download/${version}/crystalline_${version#v}_${arch}.deb"
sudo dpkg -i "crystalline_${version#v}_${arch}.deb"
crystalline --version
```

The package also ships a systemd unit, installed disabled - see [Linux server with systemd](docs/deployment.md#linux-server-with-systemd) to run the daemon as a managed service.

Windows, via MSI: download `crystalline-<version>-windows-amd64.msi` (or `crystalline-<version>-windows-arm64.msi` for Arm devices) from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest) and double-click it, or install silently with `msiexec /i <file> /qn`. The installer adds Crystalline to the system PATH and upgrades in place. Windows releases are not code signed yet, so verify against `SHA256SUMS` and confirm any SmartScreen prompt (More info > Run anyway).

Every [release](https://github.com/jordiboehme/crystalline/releases/latest) also ships the standalone `crystalline` binary for macOS (Apple Silicon and Intel), Linux (x86_64 and arm64, statically linked) and Windows (x64 and Arm64), with a `SHA256SUMS` file for verification - or build from a clone with `cargo build --release`. The macOS binaries are code signed and notarized with an Apple Developer ID, so Gatekeeper runs them without a prompt.

### Claude Code

```sh
crystalline install claude-code
```

One command wires the whole integration: MCP registration, the `SessionStart` onboarding hook, the `Stop` capture nudge (see [The learning loop](#the-learning-loop)) and the four topical skills. It is idempotent - rerun it any time and whatever is already correct is left untouched - and each part is skippable with `--skip-mcp`, `--skip-hooks` or `--skip-skills`; `--project` writes into the current repository's config instead of your global one, and `crystalline uninstall claude-code` reverses everything `install` did, leaving any hook, key or locally edited skill that is not Crystalline's own in place.

Give the agent its first domain:

```sh
mkdir -p ~/knowledge/engineering
crystalline domain init ~/knowledge/engineering --name engineering
crystalline domain add engineering ~/knowledge/engineering
```

Start a session. The agent is onboarded automatically (see [Session onboarding](#session-onboarding)) and captures what it learns as engrams from there.

### Claude Desktop

No terminal needed. Download the `.mcpb` file for your platform from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest), then in Claude Desktop open Settings > Extensions > Advanced settings > Install Extension... and pick the file. It starts with no domains: the agent creates one whenever it needs somewhere to capture knowledge, with the `add_domain` tool, as a folder of markdown files under your `Documents/Crystalline` folder, a database-backed domain or a GitHub team domain. Onboarding is automatic on every connection (see [Session onboarding](#session-onboarding)). The optional companion skill adds capture and collaboration best practices (see [Skills](#skills)); the [Claude Desktop extension scenario](docs/deployment.md#claude-desktop-extension) shows how it works underneath.

### Codex CLI

The same integration, one command (Codex keeps MCP registration user-level even with `--project`; the installer says so when it applies):

```sh
crystalline install codex
```

Then give the agent its first domain:

```sh
mkdir -p ~/knowledge/engineering
crystalline domain init ~/knowledge/engineering --name engineering
crystalline domain add engineering ~/knowledge/engineering
```

### GitHub Copilot CLI

The same integration for the agentic Copilot CLI, one command (Copilot too keeps MCP registration user-level even with `--project`). The installer drives the `copilot` binary and falls back to `gh copilot` when only the GitHub CLI form is installed:

```sh
crystalline install copilot
```

Hooks land in a dedicated `~/.copilot/hooks/crystalline.json` and skills in `~/.copilot/skills` (both honor `COPILOT_HOME`); with `--project` they go to `.github/hooks` and `.github/skills` instead, which Copilot loads once you trust the folder. Then give the agent its first domain as above.

### Any MCP harness

Crystalline runs as an MCP server over stdio; the server command is always `crystalline mcp`. Everything the installer does can also be done by hand:

```sh
claude mcp add crystalline --scope user crystalline mcp   # Claude Code, all projects
codex mcp add crystalline -- crystalline mcp              # Codex CLI
copilot mcp add crystalline -- crystalline mcp            # GitHub Copilot CLI
```

The first agent to connect starts a background daemon that loads the embedding model once and watches every registered domain; every later connection - other agents, other terminals, other harnesses - attaches to that same daemon, so there is always one shared instance and one consistent view of the index. A daemon running in a container is reached over HTTP instead of stdio - see [Run in a container](docs/deployment.md#run-in-a-container).

### From the terminal

The CLI mirrors everything an agent can do. This runs verbatim, start to finish, on a clean machine:

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

Engrams written through Crystalline are indexed immediately; `crystalline sync` only picks up files created outside it (an editor, a `git pull`) when no daemon is watching them. Edit the domain's `MANIFEST.md` `## Scope` and `## When to Use` sections so routing describes it accurately - that file is what the session prompt and an agent's routing decisions read (see [Session onboarding](#session-onboarding)).

## Session onboarding

Every MCP client is onboarded automatically: the crystalline server's instructions, returned when a client connects, carry a live routing block - one line per registered domain summarizing when to use it, plus the behavior rules (narrow question -> search that domain; broad question -> sweep all of them; writes always name a domain explicitly). The block names the exact crystalline tools each rule refers to (`search_engrams`, `write_engram` and the rest), so an agent with several MCP servers connected knows which tool on which server to call. Domain lists and file-domain MANIFESTs are read fresh for every new connection; virtual-domain routing lines follow the daemon's latest snapshot, refreshed on every stdio connection and on every local virtual write. Claude Desktop and any harness that shows the model its MCP server instructions need no further setup.

The same routing block is available outside MCP: `crystalline prompt system` renders it to stdout from every registered domain's `MANIFEST.md`, to feed to an agent as session context. Over MCP there is no workspace, so `prompt.rules` filters and repo-local `preferred_domains` apply only on this path - `crystalline prompt system --workspace .` scopes it to the current repository. `prompt` takes a subcommand naming the kind of prompt to generate; `system` is the only kind today.

The generic harness recipe: run `crystalline prompt system` at session start and inject its stdout as context before the agent does anything else. In Claude Code that is a `SessionStart` hook in `settings.json`, matched on `startup|clear|compact` so the routing block is re-injected after `/clear` and after a compaction as well as on a fresh start (a resumed session is deliberately excluded, since its transcript already carries the earlier routing block). [Get started](#get-started) covers `crystalline install`, which writes this hook for you; by hand it is:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|clear|compact",
        "hooks": [
          { "type": "command", "command": "crystalline prompt system" }
        ]
      }
    ]
  }
}
```

Any harness with an equivalent session-start hook can run the same command the same way.

### The learning loop

The other half of what `crystalline install` wires: a `Stop` hook running `crystalline hook stop` - a once-per-session, late nudge that closes the gap between an agent learning something mid-session and actually capturing it. It fires on the first `Stop` event a session reaches after real substance - a transcript past a size and line threshold, or the third `Stop` call in a row when a harness sends no transcript at all - and stays silent on every other call: below that threshold, once it has already fired for the session, when the effective mode is read-only or when no domain is registered at all. When it fires, it asks the agent to review the conversation for durable learnings and propose capturing each one into the fitting domain, the same propose-first, wait-for-a-yes shape the capture skill already follows; the reminder costs about 100 tokens, at most once per session. Remove it with `crystalline uninstall <harness>`, or leave it out from the start with `--skip-hooks`.

## Teach and learn

The MCP server exposes 14 tools, 18 once team domains are turned on (see [Share knowledge with a team](#share-knowledge-with-a-team)); capturing knowledge as a byproduct of work is the core loop:

- **`write_engram`** - capture a new engram. `domain` is always required (there is no default domain for writes, so an agent never writes into the wrong place). `permalink`, `status` and `recorded_at` are filled in for you.
- **`search_engrams`** - search before writing, and search to recall what is already known. Defaults to hybrid text-plus-semantic ranking across every domain; pass `domains` to narrow it, or filter by `type`, `tags`, `status` or arbitrary `metadata_filters` with no query text at all.
- **`edit_engram`** - refine an engram in place (`append`, `prepend`, `find_replace`, `replace_section`, `insert_before_section`, `insert_after_section`) instead of creating a duplicate for the same topic.
- **`build_context`** - given a `crystalline://domain/permalink` anchor, follow its relations and links (across domains too) to assemble the neighbourhood around a task before diving in.

Observations are the atomic unit of an engram's body: top-level bullets like `- [decision] we chose Postgres for the write path #database`. Categories are free text; useful ones include `decision`, `fact`, `pattern`, `gotcha`, `convention`, `lesson`, `risk` and `idea`. Relations connect engrams: `- depends_on [[Other Engram]]`, or `- "relates to" [[Other Engram]]` for a multi-word relation type.

Temporal fields are plain and easy to get wrong by overthinking them: an absent `valid_from` means the engram has always been valid, an absent `valid_to` means it is valid forever. When set, the fields are plain ISO dates (YYYY-MM-DD) at day granularity, and the write drops a sentinel far-future value outright, since absence already means forever. Set them only when a fact is genuinely time-bounded (a policy that changes on a known date, a temporary workaround). `status` and `type` have recommended value sets stated in the tool descriptions themselves (status: `current`, `draft`, `idea`, `deprecated`, `superseded`, and so on; type: `engram`, `guide`, `decision`, `architecture`, `runbook`, `reference`) - they exist so an agent can tell an idea apart from current fact, and they are guidance, never a global enum a write is rejected for.

The CLI mirrors the mutating and read tools directly for scripting and quick edits outside an agent session: `crystalline write`, `read`, `edit`, `move`, `delete`, `search`, `context` and `recent` take the same parameters as their MCP counterparts.

## Skills

The `skills/` folder ships four harness-agnostic agent skills plus one consolidated skill, teaching an agent how to use Crystalline well:

- **`crystalline-routing`** - which domain(s) to search for a task, when to sweep every domain instead, temporal filtering for "what is true now", and when to fall back to reading a MANIFEST directly.
- **`crystalline-capture`** - when captured knowledge is worth writing down, searching before writing to avoid duplicates, editing an existing engram instead of forking the topic, and the observation-category and temporal-field conventions that keep engrams useful later.
- **`crystalline-schema`** - authoring a Picoschema schema engram for a domain that wants structure, inferring one from what is already captured, and validating conformance.
- **`crystalline-collaboration`** - working in a domain that has a team origin: checking status at session start, updating before deep work, sharing a coherent unit of knowledge as a proposal and relaying its review URL, conflict etiquette and connecting a new teammate end to end.
- **`crystalline-memory`** - a single consolidated skill for Claude Desktop and other harnesses that install one skill at a time: recall, capture, read-only stand-down and team sharing essentials in one file.

`crystalline install claude-code` (or `codex` or `copilot`) copies these same four skills into place automatically - `~/.claude/skills` for Claude Code, `~/.agents/skills` for Codex, `~/.copilot/skills` for the Copilot CLI - and leaves `crystalline-memory` alone, since it is Claude Desktop's own consolidated skill. Each is a plain folder with a `SKILL.md`; to do it by hand instead, copy the folder into wherever your harness looks for skills. For Claude Code, that is `.claude/skills/` in a project or `~/.claude/skills/` globally:

```sh
cp -r skills/crystalline-routing skills/crystalline-capture skills/crystalline-schema skills/crystalline-collaboration ~/.claude/skills/
```

Installed skills stay current on their own: each install is recorded in a local receipt and when a new crystalline version first runs it refreshes the installed skills at session start - updating changed ones (an edited copy is kept beside the new one as `SKILL.md.bak`) and removing ones the new version no longer ships.

Installing from a release instead of a clone: download `crystalline-agent-skills-v<version>.zip` from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest) and unpack it into `~/.claude/skills/`. Zip installs are not tracked by the receipt, so re-unpack the zip after upgrading crystalline (or run `crystalline install` once to switch to managed skills).

Claude Desktop: download `crystalline-claude-desktop-skill-v<version>.zip` from the latest release, then open Settings > Capabilities > Skills (enable the Skills capability there if it is off) and upload the zip as-is (it contains the `crystalline-memory` folder; do not unpack it). Routing itself needs no skill - the server's instructions deliver it automatically; the skill adds capture and collaboration best practices.

Other harnesses that support a similar skill or instruction-file convention can point at the same folders directly; the content only assumes the MCP tools documented in [Teach and learn](#teach-and-learn), never a specific harness.

## Ship tools with a domain

Teaching an agent what a domain knows is half the story - the other half is the working tools that knowledge depends on to act on it: skills, slash commands, subagent definitions and MCP server configs. A domain's `MANIFEST.md` can declare a `## Provisioning` section naming the folders it ships, one bullet per kind:

```
## Provisioning

- skills: skills
- commands: commands
- agents: agents
- mcps: mcps
```

Each bullet is `type: path`, one of `skills`, `commands`, `agents` or `mcps` (a folder of JSON configs for `mcps`); `path` is relative to the MANIFEST itself and may climb out of the domain root with `../` to point at a folder that lives beside it. The starter MANIFEST `crystalline domain init` scaffolds does not include this section - add it by hand once a domain actually ships something. Every artifact is authored once and translated into whichever harnesses' formats allow it, a markdown agent becoming Codex's TOML dialect and back again.

Nothing ships until a person decides to: an undecided domain surfaces at session start so the agent can raise it with the person at the keyboard, then applies the answer with the `provision` MCP tool or from the terminal:

```sh
crystalline provision allow engineering   # opt in, then reconcile
crystalline provision deny engineering    # opt out, removing anything already shipped
crystalline provision status              # every domain's decision, every harness's installed state
```

Bare `crystalline provision` reconciles every opted-in domain into every harness this machine has onboarded. It is idempotent and safe to rerun - installing what is missing, updating what changed and retiring what a domain no longer ships. A provisioned file you edited by hand is still brought current on the next reconcile, with your edited version kept beside it as a `.bak` copy rather than lost; a foreign file Crystalline never wrote is adopted when it already matches byte for byte and otherwise left untouched, never overwritten.

## Share knowledge with a team

A team domain is an ordinary domain whose files also live in a GitHub repository: local markdown stays the source of truth on this machine, and an origin records which repository, subfolder and branch it tracks.

Connect this machine to GitHub once:

```sh
crystalline config set github.enabled true
crystalline connect github
```

`connect github` opens a short code to confirm at github.com/login/device, or takes a personal access token via `--token` for someone who would rather skip the browser; either way there is no git and no SSH key involved, since connecting only establishes this machine's GitHub identity. An agent does the same through the `configure` MCP tool, passing `connect: "github"` and relaying the code to the person at the keyboard.

Bring a team repository in as a domain:

```sh
crystalline domain add design --origin acme/design-knowledge --branch main
```

`--origin` takes `owner/repo` or `owner/repo/subpath` when the domain is a subfolder of a bigger repository; the local folder defaults to `<domains_root>/<name>` (the domains root is `~/Documents/Crystalline` unless you set `domains_root` or `CRYSTALLINE_DOMAINS_ROOT`) and the domain is downloaded and indexed immediately. An agent does the same with the `add_domain` MCP tool.

From there, `crystalline origin` covers the team domain lifecycle:

- **`origin status [--domain <name>]`** - where a team domain stands: ahead, behind, open and declined proposals, unresolved conflicts.
- **`origin update [--domain <name>]`** - bring a team domain (or every one) up to date with what the team has merged.
- **`origin share <name> [--title <t>] [--message <m>]`** - share local changes as a proposal the team reviews on GitHub; refuses while a conflict is unresolved so the team always reviews a clean proposal.
- **`origin resolve <name> <path> --keep mine|theirs`** (or `--content-file <f>` for a hand-merged result) - settle a flagged conflict.
- **`origin discard <name> --proposal <n>`** - abandon a declined or no-longer-wanted proposal, restoring local files that were not touched since sharing them.

The same actions are MCP tools an agent calls directly: `update_domain`, `origin_status`, `share_changes` and `resolve_conflict`, plus `configure` for settings and connecting. These four only appear once `github.enabled` is true, so an install that never uses team domains carries no extra tool beyond `configure` itself. `add_domain` is not among them: it creates domains of every kind (local, virtual, team) and is always available, though its team-domain branch still needs `github.enabled`. Sharing always ends with the agent relaying the proposal's review URL to the person it is working with, since review and merging happen on GitHub, by a person, never by the agent.

`crystalline config show`, `set <key> <value>` and `unset <key>` read and write the same settings registry the `configure` MCP tool exposes, today the `github.*` block. Every settings key also maps to a `CRYSTALLINE_*` environment variable, so a container never needs to mount this file at all - see [Configure through environment variables](docs/deployment.md#configure-through-environment-variables) for the full list. A domain's origin and the global `github` block look like this in `config.yaml`:

```yaml
domains:
  design:
    path: ~/Documents/Crystalline/design
    origin:
      repo: acme/design-knowledge   # the GitHub repository, owner/name
      path: knowledge               # optional subfolder; absent means the repository root
      branch: main                  # optional; absent means main
      poll_secs: 600                # optional per-domain poll interval override
github:
  enabled: true                     # turns team domains on; absent means off
  poll_secs: 300                    # background poll interval in seconds; minimum 60
  api_url: https://github.example.com/api/v3   # GitHub Enterprise Server only
  oauth_client_id: abc123                       # a self-hosted OAuth App, GitHub Enterprise Server only
```

## Keep knowledge honest

`crystalline verify` statically checks one or more domains against the full rule catalog - malformed frontmatter, broken links, missing MANIFEST sections, schema drift - with no database, service or network connection involved. Its usual home is CI/CD on the GitHub repositories that hold a team's knowledge: every proposal is verified before the team merges it, so nothing malformed ever lands on the branch everyone pulls from. The bundled GitHub Action wires that up:

```yaml
- uses: jordiboehme/crystalline/action@v0.8.5
  with:
    paths: knowledge/       # space-separated domain roots, default '.'
    strict: 'false'         # promote Warning rules to Error
    version: v0.8.5         # crystalline binary tag to download, or 'latest'
```

The action ref (`@v0.8.5`) pins the action's own code; `version` pins the crystalline binary it downloads, so pinning both gives a fully reproducible check. The binary is checksum-verified, then the action runs `crystalline verify`, annotates the run and, on a pull request, posts a single summary comment kept up to date in place.

Two more commands keep a knowledge base trustworthy:

- **`crystalline import <src> --domain <name>`** brings an existing markdown-plus-frontmatter knowledge base under Crystalline: normalizes legacy `type` values, backfills `status` and temporal metadata, drops sentinel far-future dates in favor of leaving the field open-ended, and adds a missing `timestamp` - all as a pure file transformation, with `--dry-run` to preview first.
- **`crystalline doctor`** diagnoses the index, registered domains and service state (orphan index rows, encoding issues, stale service locks) and repairs what it safely can with `--fix`. Once team domains are turned on it also reports whether this machine is connected to GitHub and whether each team domain's local origin state is intact. When a domain ships provisioned artifacts, it reports every declaring domain's decision and shipped counts and every installed harness's drift, locally edited and orphaned counts against what was last reconciled - that part, like the GitHub checks, is always report-only, `--fix` never reconciles a harness.

## Deployment

Crystalline runs the same way in every scenario: a daemon in the middle keeps one search index in sync with knowledge, and one or more agents connect to it, whether over a local stdio pipe or a network HTTP endpoint. [docs/deployment.md](docs/deployment.md) walks through every shape with one diagram per scenario, plus running the container image, configuring through environment variables and read-only serving.

| Scenario | In one line |
|---|---|
| [Personal workstation](docs/deployment.md#personal-workstation) | The default: local folders, agents over stdio, one shared background daemon |
| [Claude Desktop extension](docs/deployment.md#claude-desktop-extension) | One-click `.mcpb` install, no terminal involved; the agent creates domains at runtime |
| [Team server](docs/deployment.md#team-server) | One container on the network, every agent connects over HTTP |
| [Linux server with systemd](docs/deployment.md#linux-server-with-systemd) | The .deb ships a unit, disabled by default; enable it once and agents connect over HTTP |
| [Published read-only knowledge base](docs/deployment.md#published-read-only-knowledge-base) | Knowledge curated in a git repository, served read-only to agents |
| [Air-gapped or egress-restricted](docs/deployment.md#air-gapped-or-egress-restricted) | The `with-model` image or a pre-fetched model directory; nothing at runtime needs the network |
| [Shared database collaboration](docs/deployment.md#shared-database-collaboration) | Several instances share one PostgreSQL index, so every capture is visible to all |
| [Team knowledge on GitHub](docs/deployment.md#team-knowledge-on-github) | A domain tracks a GitHub repository; sharing goes through reviewed proposals |

## Virtual domains

Most domains are folders of files. A virtual domain is the other option: its engrams live in the database, with no filesystem root. Reach for one where a filesystem is baggage rather than a feature - a container with no writable volume, a PostgreSQL backend shared across machines, or a domain you would rather not mirror to disk at all.

```sh
# Register a database-backed domain and scaffold its MANIFEST into the index.
crystalline domain add notes --virtual

# It works with the same tools as any domain.
crystalline write notes "First note" --content "captured straight into the database"
crystalline search "captured"
```

Two commands move engrams between the two kinds of truth:

- `crystalline domain import <path> --domain <name>` loads already-well-formed engram files into a virtual domain, verbatim. It is distinct from `crystalline import`, which converts a legacy tree into a *file* domain's directory.
- `crystalline domain export <path> --domain <name>` writes any domain's engrams back out as a normal markdown folder. This is how you take a virtual domain's data out to run `crystalline verify` on it, or convert it back to files whenever you change your mind.

Concurrent edits to the same virtual engram are guarded: `read_engram` returns a checksum, and passing it back as `expected_checksum` on `edit_engram` refuses the edit if the engram changed since you read it, so a stale write conflicts instead of clobbering. Omit it for last-write-wins.

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

## FAQ

**When does the daemon start?**

Two ways. Explicitly: `crystalline serve` runs it in the foreground, `crystalline serve --daemon` in the background. Implicitly: the first agent that connects through `crystalline mcp` attaches to a running daemon or starts one on the spot. Either way an advisory lock guarantees a single instance; every later agent, terminal or CLI command attaches to that one.

**When does the daemon stop?**

Only when told to. It does not exit when the last agent disconnects or on idle - watching, embedding and origin polling keep running so the index stays warm for the next session. It shuts down cleanly on `crystalline ctl shutdown`, on Ctrl-C in a foreground `serve` and on SIGTERM (which is how the container image stops). On the way out it releases its host locks and removes its socket and lock files.

**How do I stop it manually?**

`crystalline ctl shutdown` from any terminal asks the running daemon to stop cleanly over the local socket. If a crash ever leaves a stale lock or socket file behind, `crystalline doctor --fix` cleans them up.

**Is the HTTP endpoint authenticated?**

Not yet - the optional HTTP transport is unauthenticated regardless of bind address. That is fine on the `127.0.0.1` default; the container image binds `0.0.0.0` (see [Run in a container](docs/deployment.md#run-in-a-container)) so agents on the host can reach it, so treat the network boundary around the container (a private network, a reverse proxy, firewall rules) as the access control until built-in authentication ships. It does validate the request `Host` header to block DNS rebinding: loopback is accepted by default, and any other hostname (a reverse proxy, a LAN name, a compose service-name) must be added via `CRYSTALLINE_SERVICE_ALLOWED_HOSTS` or `serve --allowed-host` (see [Configure through environment variables](docs/deployment.md#configure-through-environment-variables)).

**Where does my knowledge actually live?**

In your domain folders, as plain markdown you can read, edit and back up with anything. Everything Crystalline derives from it is disposable: the search index lives in the state directory and `crystalline reindex --full` rebuilds it from the files at any time. The config file, the index and the model cache live in the platform config, state and cache directories (`~/.config/crystalline`, `~/.local/state/crystalline` and `~/.cache/crystalline` on Linux and macOS).

**Do I need git to share knowledge with a team?**

No. Team domains talk to GitHub directly over its API - no git, no gh, no local clones. Members connect once with a browser code and Crystalline handles the rest.

## License

GNU Affero General Public License v3.0 - see [LICENSE](LICENSE).
