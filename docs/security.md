# Security

This page collects the facts a security review of Crystalline needs: what it does next to the agent harness, what it sends, where it listens, what it writes and how to verify a release. Known gaps are named plainly in the last section.

## What Crystalline is, next to the agent harness

Crystalline is an MCP server with a background daemon, a CLI and a web UI in one binary. The agent loop runs in a harness such as Claude Code. The two have different jobs:

| | Crystalline | The agent harness (for example Claude Code) |
|---|---|---|
| Runs a language model | No. The only model is a small local embedding model for search vectors | Yes, through its provider |
| Sends data to an LLM provider | No, unless you set a remote embedding endpoint (see below) | Yes, including everything Crystalline returns to it |
| Runs shell commands | No. There is no shell or exec tool. The only processes it starts are its own daemon, the `crystalline` on the PATH (a version check during install) and the harness CLIs (`claude`, `codex`, `copilot`) during install, uninstall and provisioning | Yes, under its own permission rules |
| Reads files | Files in registered domain folders, its own config, state and model folders, and the harness settings during install and uninstall | Whatever its permissions allow |
| Network | See [Outbound connections](#outbound-connections) | Its provider, and whatever its tools reach |

Everything Crystalline returns (search hits, engram text, the routing block) goes to the LLM provider through the harness. That includes the per-prompt recall hook, which hands the agent matching engrams on each prompt (up to three by default) while `recall.enabled` is on, which is the default.

The `--harness` argument in an MCP registration grants or denies no tool. It only decides whether the skills are served again over MCP to a harness that already has them as files.

Tool permissions for an agent are set in the harness. Claude Code, for example, has permission rules that allow or deny an MCP tool by name, such as `mcp__crystalline__delete_engram`. This is a feature of the harness, not of Crystalline.

## Outbound connections

One connection happens by default. Every other one exists only after you turn it on.

- **Hugging Face, for the embedding model.** The download starts on its own on first start, or with `crystalline model download`. It sends no engram data. The files are fetched at a fixed commit (for the default model `835ad14087e140460703cf0fae09f97d469d65c2`), not at a branch. When the cache already holds that commit, no call is made at all.
  - To use a mirror, set `HF_ENDPOINT`. To use a copy fetched ahead of time, set `CRYSTALLINE_MODELS_DIR`. The `with-model` container image has the model built in. See [Air-gapped or egress-restricted](deployment.md#air-gapped-or-egress-restricted).
  - If a Hugging Face token is present, it is sent with the download. The token is read from `HF_TOKEN`, from the file `HF_TOKEN_PATH` names, or from the token file a Hugging Face login leaves in `HF_HOME`. Set `HF_HUB_DISABLE_IMPLICIT_TOKEN` to stop that.
  - The model weights are stored with Hugging Face's Xet storage. The storage host is handed out by huggingface.co at download time. An egress allowlist that holds only `huggingface.co` may therefore not be enough. A mirror or a pre-fetched copy avoids the question.
- **GitHub** (opt-in, `github.enabled`). Team collaboration talks to `api.github.com` and to `github.com` for the sign-in device flow. `github.api_url` points it at a GitHub Enterprise server instead. While collaboration is on and connected, the daemon polls each GitHub-backed domain every `github.poll_secs` seconds (default 300, minimum 60).
- **Your single sign-on provider** (opt-in). When OIDC sign-in is configured for the web UI, Crystalline talks to the issuer you name in `auth.oidc.issuer`.
- **A remote embedding service** (opt-in). With `embeddings.provider: openai-compatible` in `config.yaml`, the text of your engrams is sent to the endpoint you configure. The default provider is local and sends nothing. This key is not in the settings registry, so an agent cannot switch it with the `configure` tool.
- **A PostgreSQL server** (opt-in). With `database.backend: postgres` and `database.url`, the search index lives in that database, and the index holds engram text. See [Shared database collaboration](deployment.md#shared-database-collaboration).

Nothing else. There is no telemetry and no update check.

## The local listener

The daemon opens one HTTP endpoint, `127.0.0.1:7411` by default. It binds loopback only. The container image is the exception: it binds `0.0.0.0:7411` inside the container.

That endpoint carries MCP over HTTP, the JSON API and the web UI.

- The JSON API and the web UI require an account. A daemon with no account yet lets a caller on loopback create the first admin. On any other bind it asks for a one-time setup token that it prints at startup.
- MCP over HTTP is open by default (`auth.mcp` is `false`). Any process that can reach the port is served without a token. It can read and write every domain that is not private, and it can change settings, register domains and apply provisioning. On a shared host this includes processes of other users.
- The HTTP transport accepts only loopback `Host` headers by default, as a guard against DNS rebinding. `service.allowed_hosts` adds more names.
- There is no built-in TLS. For anything other than localhost, put a TLS terminator in front (see [Web UI from the daemon](deployment.md#web-ui-from-the-daemon)).

How to close it:

- `crystalline config set auth.mcp true`: every MCP connection over HTTP then needs a personal token, issued in Fluid or from the CLI (see [Authenticated agents](deployment.md#authenticated-agents)).
- `crystalline config set service.http false`: no HTTP endpoint at all, if your agents only use stdio. The web UI goes away with it.

Both apply the next time the daemon starts. The config file is per user, so the setting holds for every daemon that user starts.

The daemon also has a local control channel: a Unix socket (`service.sock` in the state directory) on macOS and Linux, and a named pipe on Windows. It carries the stdio MCP sessions that `crystalline mcp` relays to the daemon, and the operator commands of the CLI, for example `sync`, `status`, `reindex`, `configure`, `tool` and `shutdown`. A stdio session is treated as the machine owner and passes every access check.

## Hardening settings

Set these with `crystalline config set <key> <value>`. Every one of them is in the settings registry, so an agent can also change them with the `configure` tool: over stdio, and over HTTP while `auth.mcp` is off or when the agent's account is an admin. A read-only daemon refuses every settings change.

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
| `service.allowed_hosts` | Extra `Host` header values the HTTP endpoint accepts; `*` accepts any | loopback only |

`auth.mcp`, `service.http`, `service.read_only`, `skills.serve`, `auth.anonymous` and `service.allowed_hosts` apply the next time the daemon starts. The others apply at once.

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
  - `service.lock`, `service.json`, `service.sock` and `daemon.log`;
  - `tmp/`, the daemon's scratch folder;
  - `origins/`, the state of team domains, and `origins/github-token.json` only when the OS keychain cannot be used (on Windows this file gets no extra access restriction; on macOS and Linux it is readable by the owner only);
  - `overlays/`, the review-mode drafts;
  - `instance-id`, `installs.json` and `hooks/maintenance.json`.
- The GitHub token is otherwise kept in the OS keychain (Credential Manager on Windows).

Nothing is encrypted at rest. Use disk encryption.

`crystalline install claude-code` registers the MCP server with the `claude` CLI, adds three hooks to `~/.claude/settings.json` (`SessionStart`, `Stop` and `UserPromptSubmit`) and copies four skills into `~/.claude/skills`. With `--project` it writes into the project's `.claude/` folder instead. Hook entries of other tools are kept. `crystalline uninstall claude-code` removes what install added.

## Verify a release

Every release has a `SHA256SUMS` file that lists every other asset.

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

On Windows, run `Get-FileHash <file>` in PowerShell and compare the hash with the line in `SHA256SUMS` by hand.

Since 0.21.2, every release asset, `SHA256SUMS` included, has a build provenance attestation. It proves that the file was built by this repository's release workflow from the tagged commit. Verify it with the GitHub CLI:

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

How a release is built:

- On GitHub-hosted runners, from the tagged commit.
- Every action in the build and release workflows is pinned by commit.
- `cargo build --locked` with the committed `Cargo.lock`. WiX, the mcpb packer and the cargo tools are pinned to exact versions.
- Dependencies are audited in CI on every pull request and every push to main: `cargo-deny` with `deny.toml` checks RustSec advisories, a license allowlist and crates.io as the only source, and `pnpm audit --prod` checks the web UI's runtime dependencies.

The Windows binaries and the MSI are not Authenticode signed yet. SmartScreen warns, and AppLocker or WDAC can allow them only by file hash. The macOS binaries are signed and notarized.

## License

Crystalline is licensed under the GNU AGPL v3.0 or later (see [LICENSE](../LICENSE)). The third-party licenses the binary may contain are the allowlist in [`deny.toml`](../deny.toml), and the SBOMs list each component with its license.

## Known gaps

- MCP over HTTP is open to any local process by default (`auth.mcp` is `false`).
- The Windows binaries and the MSI have no Authenticode signature.
- On Windows the config and state folders, including the search index and the accounts database, are in the roaming profile.
- The generated `index.md` files are rewritten and removed on their own while `index.files` is on.
- `provision` can register MCP servers, skills and agents that a domain's MANIFEST ships into your harnesses, and the server asks for no confirmation.
- `add_domain` accepts any folder the account can read.
- There is no server-side allowlist of tools and no read-only flag per local domain. Read-only is daemon-wide.
- There is no encryption at rest and no built-in TLS.
- The builds are not reproducible.

These are tracked, and a report about any of them is welcome (see [SECURITY.md](../SECURITY.md)).
