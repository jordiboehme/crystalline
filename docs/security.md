# Security

This page collects the facts a security review of Crystalline needs: what it does next to the agent harness, what it sends, where it listens, what it writes and how to verify a release. Known gaps are named plainly in the last section.

## What Crystalline is, next to the agent harness

Crystalline is an MCP server with a background daemon, a CLI and a web UI in one binary. The agent loop runs in a harness such as Claude Code. The two have different jobs:

| | Crystalline | The agent harness (for example Claude Code) |
|---|---|---|
| Runs a language model | No. The only models are small local ones that generate no text: an embedding model for search vectors and, while `evolve.contradictions` is on, a classifier that reads two lines and scores whether they contradict | Yes, through its provider |
| Sends data to an LLM provider | No, unless you set a remote embedding endpoint (see below) | Yes, including everything Crystalline returns to it |
| Runs shell commands | No. There is no shell or exec tool. The only processes it starts are its own daemon, the `crystalline` on the PATH (a version check during install) and the harness CLIs (`claude`, `codex`, `copilot`, or `gh copilot` as the fallback) during install, uninstall and provisioning | Yes, under its own permission rules |
| Reads files | Files in registered domain folders, its own config, state and model folders, and the harness settings during install and uninstall | Whatever its permissions allow |
| Network | See [Outbound connections](#outbound-connections) | Its provider, and whatever its tools reach |

Everything Crystalline returns (search hits, engram text, the routing block) goes to the LLM provider through the harness. That includes the per-prompt recall hook, which hands the agent matching engrams on each prompt (up to three by default) while `recall.enabled` is on, which is the default.

The `--harness` argument in an MCP registration grants or denies no tool. It only decides whether the skills are served again over MCP to a harness that already has them as files.

Tool permissions for an agent are set in the harness. Claude Code, for example, has permission rules that allow or deny an MCP tool by name, such as `mcp__crystalline__delete_engram`. This is a feature of the harness, not of Crystalline.

## Outbound connections

One connection happens by default. Every other one exists only after you turn it on.

- **Hugging Face, for the embedding model.** The download starts on its own on first start, or with `crystalline model download`. It sends no engram data. The files are fetched at a fixed commit (for the default model `835ad14087e140460703cf0fae09f97d469d65c2`), not at a branch. When the cache already holds that commit, no call is made at all. When it holds only an older snapshot of the same model, that one is used with no call at startup, and a daemon then tries once per start, in the background, to fetch the fixed commit.
  - To use a mirror, set `HF_ENDPOINT`. To use a copy fetched ahead of time, set `CRYSTALLINE_MODELS_DIR`. The `with-model` container image has the model built in. See [Air-gapped or egress-restricted](deployment.md#air-gapped-or-egress-restricted).
  - If a Hugging Face token is present, it is sent with the download. The token is read from `HF_TOKEN`, from the file `HF_TOKEN_PATH` names, or from the token file a Hugging Face login leaves in `HF_HOME`. Set `HF_HUB_DISABLE_IMPLICIT_TOKEN` to stop that.
  - The model weights are stored with Hugging Face's Xet storage. The storage host is handed out by huggingface.co at download time. An egress allowlist that holds only `huggingface.co` may therefore not be enough. A mirror or a pre-fetched copy avoids the question.
- **Hugging Face, for the contradiction model** (opt-in, `evolve.contradictions`). With the setting on `full`, the daemon downloads an NLI checkpoint on its first pass, from the same host, into the same cache and with the same token and mirror rules as the embedding model, at a fixed commit (`b5113eb38ab63efdd7f280f8c144ea8b13f978ce`). It sends no engram data. No container image bundles it. The key is in the settings registry, so an agent that may change settings can turn it on with the `configure` tool.
- **GitHub** (opt-in, `github.enabled`). Team collaboration talks to `api.github.com`, to `codeload.github.com` for repository downloads (the API redirects archive downloads there, which is GitHub behaviour), and to `github.com` for the sign-in device flow. `github.api_url` points it at a GitHub Enterprise server instead. While collaboration is on and connected, the daemon polls each GitHub-backed domain every `github.poll_secs` seconds (default 300, minimum 60).
- **Your single sign-on provider** (opt-in). When OIDC sign-in is configured for the web UI, Crystalline talks to the issuer you name in `auth.oidc.issuer`. The token endpoint and the key set come from the issuer's discovery document and can be on other hosts of the provider.
- **A remote embedding service** (opt-in). With `embeddings.provider: openai-compatible` in `config.yaml`, the text of your engrams is sent to the endpoint you configure. The default provider is local and sends nothing. This key is not in the settings registry, so an agent cannot switch it with the `configure` tool.
- **A PostgreSQL server** (opt-in). With `database.backend: postgres` and `database.url`, the search index lives in that database, and the index holds engram text. These keys are set with `crystalline config set` or their environment variables; no agent can change them, and the change applies at the next daemon start. See [Shared database collaboration](deployment.md#shared-database-collaboration).
- **A connected Crystalline server** (opt-in). After `crystalline connect <url>`, or with `CRYSTALLINE_REMOTE_URL` and `CRYSTALLINE_REMOTE_TOKEN` set, the daemon talks to that server with your token for it. A call for one of the server's domains goes to that server only, with what the call carries (for a write, the engram text). A search over all domains also sends the search text to every connected server. Your local engrams are never sent there. A small cache of each server's routing text and maintenance status stays on this machine. Sources are not in the settings registry, so an agent cannot add one with the `configure` tool. See [A local Crystalline with domains from shared servers](deployment.md#a-local-crystalline-with-domains-from-shared-servers).

Nothing else. There is no telemetry and no update check.

## The local listener

The daemon opens one HTTP endpoint, `127.0.0.1:7411` by default. It binds loopback only. The container image is the exception: it binds `0.0.0.0:7411` inside the container.

That endpoint carries MCP over HTTP, the JSON API and the web UI.

- The JSON API and the web UI require an account. A daemon with no account yet lets a caller on loopback create the first admin. On any other bind it asks for a one-time setup token that it prints at startup.
- MCP over HTTP is open by default (`auth.mcp` is `false`). Any process that can reach the port is served without a token. It can read and write every domain that is not private, and it can change settings, register domains and apply provisioning. On a shared host this includes processes of other users.
- The `Host` allow-list guards the MCP endpoint and the OAuth endpoints, loopback only by default, as a guard against DNS rebinding. `service.allowed_hosts` adds more names. A browser reaching the web UI and the JSON API needs no entry; those need an account instead.
- There is no built-in TLS. For anything other than localhost, put a TLS terminator in front (see [Web UI from the daemon](deployment.md#web-ui-from-the-daemon)).

How to close it:

- `crystalline config set auth.mcp true`: every MCP connection over HTTP then needs a personal token, issued in Fluid or from the CLI (see [Authenticated agents](deployment.md#authenticated-agents)).
- `crystalline config set service.http false`: no HTTP endpoint at all, if your agents only use stdio. The web UI goes away with it.

Both apply the next time the daemon starts. The config file is per user, so the setting holds for every daemon that user starts. An environment variable of the same key (`CRYSTALLINE_AUTH_MCP`, `CRYSTALLINE_SERVICE_HTTP`) wins over the config file; the container image sets `CRYSTALLINE_SERVICE_HTTP=0.0.0.0:7411`.

The daemon also has a local control channel: a Unix socket (`service.sock` in the state directory) on macOS and Linux, and a named pipe on Windows. It carries the stdio MCP sessions that `crystalline mcp` relays to the daemon, and the operator commands of the CLI, for example `sync`, `status`, `reindex`, `configure`, `tool` and `shutdown`. A stdio session is treated as the machine owner and passes every access check.

## Hardening settings

Set these with `crystalline config set <key> <value>` or their `CRYSTALLINE_*` variables. They are operator settings: the `configure` tool does not list them and refuses to change them, over stdio and over HTTP, for an admin's agent too. So no agent can turn token checks off, open the network bind or point the search index at another database. Watch `config.yaml` for changes you did not make. A read-only daemon refuses every settings change.

With `auth.mcp` off, an agent may set a domain's policies and rule overrides through `configure` wherever it may edit that domain's MANIFEST.

| Setting | Effect | Default |
|---|---|---|
| `auth.mcp` | `true` requires a personal token for every MCP connection over HTTP | `false` |
| `service.http` | `false` turns the HTTP endpoint off; a `host:port` value binds that address | `127.0.0.1:7411` |
| `service.read_only` (or `--read-only`) | Serves the knowledge read-only: write tools leave the tool list and are refused when called. Daemon-wide, not per domain; host CLI commands such as `import` still write. See [Read-only deployments](deployment.md#read-only-deployments) | `false` |
| `github.enabled` | Turns GitHub team collaboration on; while off, the six collaboration tools are not listed and the daemon does not poll GitHub | `false` |
| `index.files` | Keeps a generated `index.md` in every folder of a file domain (see below) | `true` |
| `recall.enabled` | The per-prompt hook hands the agent matching engrams | `true` |
| `skills.serve` | Serves the agent skills over MCP; `auto` skips a stdio session whose harness already has them as files | `auto` |
| `auth.anonymous` | `true` serves JSON API requests with no identity at viewer level | `false` |
| `service.allowed_hosts` | Extra `Host` header values the MCP endpoint and the OAuth endpoints accept; `*` accepts any | loopback only |
| `service.ui` | `false` stops serving the web UI | `true` |
| `service.api` | `false` stops serving the JSON API, and the web UI with it | `true` |

`auth.mcp`, `service.http`, `service.read_only`, `skills.serve`, `auth.anonymous`, `service.allowed_hosts`, `service.ui` and `service.api` apply the next time the daemon starts. The others apply at once.

## What Crystalline writes

In a registered domain folder:

- Engram files, through writes, edits, moves, splits and deletes by an agent or in the web UI. A delete removes the file. There is no recycle bin.
- `MANIFEST.md`, when a folder is registered that has none.
- Attachments under `assets/`.
- Generated `index.md` files, one in every folder that holds knowledge. This happens on its own, after every write and after every sync that picked up outside changes. The pass overwrites any `index.md` whose content differs, a hand-written one included, and removes `index.md` from folders that hold no knowledge. Turn it off with `crystalline config set index.files false`. A read-only daemon skips it.

A team domain on GitHub also receives the merged content of every pull from its repository.

In its own folders:

| Folder | macOS and Linux | Windows |
|---|---|---|
| Config | `~/.config/crystalline` | `%APPDATA%\crystalline` |
| State | `~/.local/state/crystalline` | `%APPDATA%\crystalline` |
| Model cache | `~/.cache/crystalline/models` | `%LOCALAPPDATA%\crystalline\models` |

On macOS and Linux the `XDG_*` variables move these folders. On Windows, config and state are in the roaming profile.

- The config folder holds `config.yaml`. An OIDC client secret or a `database.url` password set with `crystalline config set` is stored there in plain text. The environment variables `CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET` and `CRYSTALLINE_DATABASE_URL` keep them out of the file.
- The state folder holds:
  - `index.db`, the search index, which holds engram text;
  - `web-auth.db`, the accounts and sessions;
  - `service.lock`, `service.json` and `daemon.log`, and `service.sock` on macOS and Linux;
  - `tmp/`, the daemon's scratch folder;
  - `origins/`, the state of team domains, and `origins/github-token.json` (plus `origins/github-token-personal-<name>.json` for personal GitHub identities) only when the OS keychain cannot be used (on Windows this file gets no extra access restriction; on macOS and Linux it is readable by the owner only);
  - `overlays/`, the review-mode drafts;
  - `instance-id`, `installs.json` and `hooks/maintenance.json`.
- The GitHub token is otherwise kept in the OS keychain (Credential Manager on Windows).

Nothing is encrypted at rest. Use disk encryption.

`crystalline install claude-code` registers the MCP server with the `claude` CLI, adds three hooks to `~/.claude/settings.json` (`SessionStart`, `Stop` and `UserPromptSubmit`) and copies five skills into `~/.claude/skills`. With `--project` it writes the hooks and skills into the project's `.claude/` folder and registers the MCP server with project scope instead. Hook entries of other tools are kept. `crystalline uninstall claude-code` removes what install added.

## Verify a release

Every release has a `SHA256SUMS` file that lists every other asset.

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

On macOS the command is `shasum -a 256 -c SHA256SUMS --ignore-missing`. On Windows, run `Get-FileHash <file>` in PowerShell and compare the hash with the line in `SHA256SUMS` by hand.

Since 0.21.2, every release asset, `SHA256SUMS` included, has a build provenance attestation. It proves that the file was built by this repository's release workflow from the tagged commit. Verify it with the GitHub CLI. `gh` must be signed in (`gh auth login`) for this and for the `gh release verify` commands below; offline, download the attestation with `gh attestation download` and pass it with `--bundle`.

```sh
gh attestation verify crystalline-v0.21.2-windows-amd64.msi --repo jordiboehme/crystalline
```

Each release also has CycloneDX 1.5 SBOMs in JSON: `crystalline-<version>-<platform>.cdx.json` for the binary of each platform, and `crystalline-fluid-<version>.cdx.json` for the web UI that every binary embeds. The SBOMs are attested against the files that carry that binary, so you verify an SBOM attestation on the MSI, zip, archive, `.deb` or `.mcpb` itself:

```sh
gh attestation verify crystalline-v0.21.2-windows-amd64.msi --repo jordiboehme/crystalline --predicate-type https://cyclonedx.org/bom
```

The container images `crystalline`, its `with-model` variant and `crystalline-fluid` are attested in the registry. Image tags carry no `v`:

```sh
gh attestation verify oci://ghcr.io/jordiboehme/crystalline:0.21.2 --repo jordiboehme/crystalline
```

Since 0.21.2, releases are immutable: once a release is published, its files and its tag cannot change. GitHub attests each immutable release as well. Check the release as a whole, or one downloaded file against it:

```sh
gh release verify v0.21.2 --repo jordiboehme/crystalline
gh release verify-asset v0.21.2 crystalline-v0.21.2-windows-amd64.msi --repo jordiboehme/crystalline
```

How a release is built:

- On GitHub-hosted runners, from the tagged commit.
- Every action in the workflows is pinned by commit.
- `cargo build --locked` with the committed `Cargo.lock`. WiX, the mcpb packer and the cargo tools are pinned to exact versions. The mcpb packer's own npm dependencies are not pinned; they are resolved when the release is built.
- Dependencies are audited in CI on every pull request and every push to main: `cargo-deny` with `deny.toml` checks RustSec advisories, a license allowlist and crates.io as the only source, and `pnpm audit --prod` checks the web UI's runtime dependencies. Ignored advisories are listed with a dated reason in `deny.toml`.

The Windows binaries and the MSI are not Authenticode signed yet. SmartScreen warns, and AppLocker or WDAC cannot allow them by a publisher rule (general Windows behaviour for unsigned files). The macOS binaries are signed and notarized.

## License

Crystalline is licensed under the Mozilla Public License 2.0 (see [LICENSE](../LICENSE)). The third-party licenses the binary may contain are the allowlist in [`deny.toml`](../deny.toml). The SBOMs list the dependencies that ship, with their versions and, where a package declares one, its license. The Rust SBOMs also name the build-time crates and mark them as excluded, because they are not part of the binary.

## Known gaps

- MCP over HTTP is open to any local process by default (`auth.mcp` is `false`).
- The Windows binaries and the MSI have no Authenticode signature.
- On Windows the config and state folders, including the search index and the accounts database, are in the roaming profile.
- The generated `index.md` files are rewritten and removed on their own while `index.files` is on.
- `provision` can register MCP servers, skills and agents that a domain's MANIFEST ships into your harnesses, and the server asks for no confirmation.
- `add_domain` accepts any folder the daemon's user can read.
- An agent that may change settings can change every key in the settings registry, including `auth.mcp`, `service.http` and the database connection. There is no confirmation step.
- There is no server-side allowlist of tools and no read-only flag per local domain. Read-only is daemon-wide.
- There is no encryption at rest and no built-in TLS.
- The builds are not reproducible.

These are tracked, and a report about any of them is welcome (see [SECURITY.md](../SECURITY.md)).
