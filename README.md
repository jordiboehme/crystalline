```
                                             ◆───◆───◆
                                            ╱ ╲ ╱ ╲ ╱ ╲
                                           ◆───◆───◆───◇
                                          ╱ ╲ ╱ ╲ ╱ ╲ ╱ ╲
                                 ◆───◆───◆╌╌╌◆╌╌╌◆───◇───◇
                                ╱ ╲ ╱ ╲ ╱ · · · · · ╱ ╲ ╱
                               ◆───◆───◆───◇╌╌╌·╌╌╌◇───◇
                              ╱ ╲ ╱ ╲ ╱ ╲ ╱ · · · · · ╱
                             ◆╌╌╌◆╌╌╌◆───◇───◆╌╌╌◆╌╌╌◆
                              · · · · · ╱ ╲ ╱ ╲ ╱ ╲ ╱ ╲
                               ·╌╌╌·╌╌╌◇───◆───◆───◆───◇
                                · · · · · ╱ ╲ ╱ ╲ ╱ ╲ ╱ ╲
                                 ·╌╌╌·╌╌╌◆╌╌╌◆╌╌╌◆───◇───◇
                                          · · · · · ╱ ╲ ╱
                                           ·╌╌╌·╌╌╌◇───◇
                                            · · · · · ╱
                                             ·╌╌╌·╌╌╌◇

 ░░░░░░╗░░░░░░╗ ░░╗   ░░╗░░░░░░░╗░░░░░░░░╗ ░░░░░╗ ░░╗     ░░╗     ░░╗░░░╗   ░░╗░░░░░░░╗
▒▒╔════╝▒▒╔══▒▒╗╚▒▒╗ ▒▒╔╝▒▒╔════╝╚══▒▒╔══╝▒▒╔══▒▒╗▒▒║     ▒▒║     ▒▒║▒▒▒▒╗  ▒▒║▒▒╔════╝
▓▓║     ▓▓▓▓▓▓╔╝ ╚▓▓▓▓╔╝ ▓▓▓▓▓▓▓╗   ▓▓║   ▓▓▓▓▓▓▓║▓▓║     ▓▓║     ▓▓║▓▓╔▓▓╗ ▓▓║▓▓▓▓▓╗
██║     ██╔══██╗  ╚██╔╝  ╚════██║   ██║   ██╔══██║██║     ██║     ██║██║╚██╗██║██╔══╝
╚██████╗██║  ██║   ██║   ███████║   ██║   ██║  ██║███████╗███████╗██║██║ ╚████║███████╗
 ╚═════╝╚═╝  ╚═╝   ╚═╝   ╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═══╝╚══════╝
```

[![CI](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml/badge.svg)](https://github.com/jordiboehme/crystalline/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/github/license/jordiboehme/crystalline)](LICENSE)
[![Latest release](https://img.shields.io/github/v/release/jordiboehme/crystalline)](https://github.com/jordiboehme/crystalline/releases/latest)
[![OKF BundleDex](https://bundledex.net/static-badge.svg)](https://bundledex.net)

**Crystalline intelligence for AI agents. Plain markdown underneath.**

Psychology splits intelligence in two: fluid intelligence reasons about a new problem in the moment, crystallized intelligence is what learning left behind, the vocabulary, the judgment and the lessons already paid for. A language model is fluid intelligence in its purest form, brilliant in the moment and a stranger at the start of every session. Crystalline is the other half: the crystalline intelligence an agent builds up and keeps, session by session, until it stops being a stranger and becomes a peer.

The difference it makes, in one exchange:

```text
Moving day
  You:    Here's the washing machine's manual as a PDF. Learn it - I am
          never reading 60 pages about laundry.
  Agent:  Learned it. Captured programs, error codes and maintenance
          into home (#appliances), manual attached.

Eight months later, a fresh session
  You:    The machine is blinking E18 and the display is in Italian??
  Agent:  Recalled from home: E18 is a blocked drain pump filter.
          Front panel, bottom right, quarter turn - towel down first,
          about a liter of water comes out.
```

## Get started

Crystalline is one binary. Two commands on a Mac with [Homebrew](https://brew.sh) and Claude Code:

```sh
brew install jordiboehme/tap/crystalline
crystalline install claude-code
```

Start Claude Code and say:

> Add two domains: books for what I read, and work for this repository. Here is my reading log from the last three years - work out what I actually like. Then capture what this repository is about.

The agent creates both domains with its `add_domain` tool, a folder with a starter MANIFEST each, captures what it learns from the log and from the repository as engrams, and the next session starts from them.

Fluid, the web UI, is at http://localhost:7411 and the first visit creates your admin account. What the install command wired, and how to undo it: [Claude Code setup](docs/setup/claude-code.md).

<details>
<summary>Linux</summary>

Via `.deb` package (Debian, Ubuntu and derivatives, amd64 or arm64):

```sh
version=$(curl -fsSL https://api.github.com/repos/jordiboehme/crystalline/releases/latest | grep -m1 '"tag_name"' | cut -d '"' -f4)
arch=amd64   # or arm64
curl -fsSLO "https://github.com/jordiboehme/crystalline/releases/download/${version}/crystalline_${version#v}_${arch}.deb"
sudo dpkg -i "crystalline_${version#v}_${arch}.deb"
crystalline --version
```

The package ships a systemd unit, installed disabled: see [Linux server with systemd](docs/deployment.md#linux-server-with-systemd) to run the daemon as a service. Then `crystalline install claude-code` and the prompt above.

</details>

<details>
<summary>Windows</summary>

Via MSI: download `crystalline-<version>-windows-amd64.msi` (or `crystalline-<version>-windows-arm64.msi` for Arm devices) from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest) and double-click it, or install silently with `msiexec /i <file> /qn`. The installer adds Crystalline to the system PATH and upgrades in place. Windows releases are not code signed yet, so verify against `SHA256SUMS` and confirm the SmartScreen prompt (More info > Run anyway).

Every [release](https://github.com/jordiboehme/crystalline/releases/latest) also ships the standalone `crystalline` binary for macOS (Apple Silicon and Intel), Linux (x86_64 and arm64, statically linked) and Windows (x64 and Arm64), with a `SHA256SUMS` file, or build from a clone with `cargo build --release`. The macOS binaries are code signed and notarized, so Gatekeeper runs them without a prompt.

</details>

<details>
<summary>Other harnesses and clients</summary>

| Client | Setup |
|---|---|
| Claude Desktop | [A one-click extension, no terminal](docs/setup/claude-desktop.md) |
| Codex CLI | [`crystalline install codex`](docs/setup/codex.md) |
| GitHub Copilot CLI | [`crystalline install copilot`](docs/setup/copilot.md) |
| Any MCP harness | [`crystalline mcp` over stdio, wired by hand](docs/setup/mcp-harness.md) |
| Remote clients | [A standing instruction for chat surfaces and the Messages API](docs/setup/remote-clients.md) |
| From the terminal | [The CLI mirrors everything an agent can do](docs/setup/terminal.md) |

</details>

## Why not a memory feature

A chat memory is a hidden blob. One vendor owns it, it is tied to one model, and nobody around the agent can see it, review it, version it or share it. Crystalline is files you own: plain markdown, read by every agent and every person on the team, reviewed like code and kept in your own repositories. Change the model and the knowledge stays.

## What sets it apart

- **Plain markdown, an open format.** An engram is a markdown file with YAML frontmatter in [Google's Open Knowledge Format (OKF) v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/tree/main/okf). Any tool reads it, nothing locks you in and the index is disposable: `crystalline reindex --full` rebuilds it from the files.
- **Scales past a folder of files.** Domains with MANIFEST routing, hybrid text-plus-semantic search, a knowledge graph and temporal filtering: the ten-thousandth engram is as findable as the tenth.
- **Knowledge retires, it does not disappear.** A fact that stopped holding is superseded, not overwritten. The old engram stays addressable by date ("what applied last June"), retired knowledge fades in ranking instead of vanishing and `crystalline evolve` tells you what the archive needs next.
- **Agents and people share one intelligence.** Agents work over MCP, people work in Fluid, and a team shares through GitHub pull requests, with review mode and private drafts for a domain that wants a gate.
- **Hello · Hallo · Hola · Bonjour · Ciao · Olá · Hoi · Ahoj · مرحبا · こんにちは · 안녕하세요 · 你好.** Search is multilingual: the built-in embedding model reads more than 200 languages, 52 of them with retrieval training, so a German engram answers an English question and the other way round, and a domain that mixes languages needs no translation.

## Fluid

The primary author in Crystalline is the agent. Fluid is how you take part: read what was learned, correct a fact, add knowledge of your own, in the browser, without spending a token. It is built into the binary and on by default at `http://localhost:7411`. An engram is a page, the editor previews live, everyone in the same engram sees each other's cursors, search is the same hybrid ranking the agents use and a maintenance page shows what the knowledge needs next. The full tour: [Fluid, the web UI](docs/fluid.md).

![An engram in Fluid: frontmatter details, observations, typed relations and the agent's-eye view](assets/fluid-engram.jpg)

## Deployment

Crystalline runs the same way in every scenario: a daemon in the middle keeps one search index in sync with knowledge, and one or more agents connect to it, whether over a local stdio pipe or a network HTTP endpoint. [docs/deployment.md](docs/deployment.md) walks through every shape with one diagram per scenario, plus running the container image, configuring through environment variables and read-only serving.

| Scenario | In one line |
|---|---|
| [Personal workstation](docs/deployment.md#personal-workstation) | The default: local folders, agents over stdio, one shared background daemon |
| [Claude Desktop extension](docs/deployment.md#claude-desktop-extension) | One-click `.mcpb` install, no terminal involved; the agent creates domains at runtime |
| [Team server](docs/deployment.md#team-server) | One container on the network, every agent connects over HTTP |
| [Web UI from the daemon](docs/deployment.md#web-ui-from-the-daemon) | The browser UI ships built into the binary, on by default at localhost - browse what your agents know with zero extra moving parts, and follow the links they hand you into it |
| [Team server with Fluid](docs/deployment.md#team-server-with-fluid) | The scale-out variant: nginx replicas in front when one daemon port is not enough |
| [Linux server with systemd](docs/deployment.md#linux-server-with-systemd) | The .deb ships a unit, disabled by default; enable it once and agents connect over HTTP |
| [Published read-only domains](docs/deployment.md#published-read-only-domains) | Knowledge curated in a git repository, served read-only to agents |
| [Air-gapped or egress-restricted](docs/deployment.md#air-gapped-or-egress-restricted) | The `with-model` image or a pre-fetched model directory; nothing at runtime needs the network |
| [Shared database collaboration](docs/deployment.md#shared-database-collaboration) | Several instances share one PostgreSQL index, so every capture is visible to all of them that registered the domain |
| [Team knowledge on GitHub](docs/deployment.md#team-knowledge-on-github) | A domain tracks a GitHub repository; sharing goes through reviewed proposals, or straight to the branch where the domain says so |
| [Authenticated agents](docs/deployment.md#authenticated-agents) | HTTP MCP requires a personal token per agent, issued in Fluid or from the CLI |
| [MCP clients over OAuth](docs/deployment.md#mcp-clients-over-oauth) | A hosted client such as Claude.ai signs the person in and consents once; no token is pasted |
| [Enterprise SSO](docs/deployment.md#enterprise-sso) | Sign in through an OpenID Connect provider; an account is provisioned on first sign-in |
| [Proxy forward auth](docs/deployment.md#proxy-forward-auth) | A forward-auth proxy (Authelia, oauth2-proxy) names the signed-in person in a header quartet |

## Go deeper

- The Crystalline Handbook is the book-length guide: the idea, the system, installation and the full working loop, written to be read in an evening or to run a training from. Read it online at https://jordiboehme.github.io/crystalline/, or download the [PDF](https://raw.githubusercontent.com/jordiboehme/crystalline/handbook-downloads/crystalline-handbook.pdf), the [EPUB](https://raw.githubusercontent.com/jordiboehme/crystalline/handbook-downloads/crystalline-handbook.epub) or the single-file [Markdown](https://raw.githubusercontent.com/jordiboehme/crystalline/handbook-downloads/crystalline-handbook.md) edition.
- [The docs](docs/README.md): setup per harness, how an agent learns, teams, verify and evolve, virtual domains, architecture.
- [FAQ](docs/faq.md): the short answers, and why not just a folder of files.
- [Deployment](docs/deployment.md): every scenario from a laptop to an air-gapped server, one diagram each.
- Found a rough edge or a missing piece? [Open an issue](https://github.com/jordiboehme/crystalline/issues) - and if Crystalline made your agent a better peer, a star helps others find it.

## Support

Crystalline is free and open source. If it earned its place in your workflow, you can support the work here:

[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/V7V31T6CL9)

## Privacy Policy

Crystalline is local-first: no telemetry, no analytics, no accounts and no data collection by the developer. Every engram lives as a markdown file plus a local search index on your own machine, entirely under your control.

Two outbound connections exist, each opt-in and user-initiated - nothing else ever leaves the machine:

- **GitHub**, only once you turn on team collaboration (`crystalline config set github.enabled true` and `crystalline connect github`). It uses your own OAuth token, and engram data flows only to the repositories you choose to share it with - governed by [GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).
- **Hugging Face**, for a one-time download of the local embedding model, automatic on first start or explicit via `crystalline model download` - governed by the [Hugging Face privacy policy](https://huggingface.co/privacy).

The developer shares nothing with anyone. Data retention is entirely user-controlled: deleting a domain or an engram deletes the data, and uninstalling Crystalline leaves your markdown untouched.

Questions: jordi@boehme-lopez.de.

## License

GNU Affero General Public License v3.0 - see [LICENSE](LICENSE).
