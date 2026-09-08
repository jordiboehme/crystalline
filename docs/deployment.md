# Deploy Crystalline

Crystalline runs the same way in every scenario: a daemon in the middle keeps one search index in sync with knowledge, and one or more agents connect to it, whether that connection is a local stdio pipe or a network HTTP endpoint. A browser is one more client of that same endpoint, since the daemon serves the web UI itself. The scenarios below are variations on that one architecture.

See [Get started](../README.md#get-started) in the README to install the binary and wire up an agent; this guide covers where the daemon, its knowledge and its index live in each shape.

## Personal workstation

The default shape: install the binary, point one or more agents at `crystalline mcp` over stdio, and the first connection spawns a background daemon that loads the embedding model once and watches every registered domain. Knowledge lives in ordinary local folders, read-write, so capturing what an agent learns as it works is the entire point. See [Get started](../README.md#get-started) in the README for the stdio setup.

That daemon serves the web UI too, however it started: a hand-typed `crystalline serve` and the one an agent's stdio connection spawns behave the same, and both open the HTTP endpoint at `http://localhost:7411` unless it is turned off. So there is nothing to deploy to browse what your agents have learned - open that address, and the first visit to an instance with no accounts yet asks you to create your admin account right there in the browser (see [Web UI from the daemon](#web-ui-from-the-daemon)). `crystalline config set service.http false` closes the endpoint for every daemon that user starts, since the config file is per user rather than per machine, and `CRYSTALLINE_SERVICE_HTTP=false` closes it for the one process it is set on.

```mermaid
flowchart LR
    A1[Agent] -->|stdio| D[Daemon]
    A2[Agent] -->|stdio| D
    B[Browser] -->|HTTP :7411| D
    D --> K[Knowledge files]
    D --> I[Index]
```

### Daemon lifecycle

It does not matter who or where starts the daemon. The first `crystalline mcp` connection spawns it as a fully detached process (its own session, no controlling terminal, stdio closed) that serves the user's state directory and outlives every client that connects to it; later connections and plain CLI verbs attach to the same daemon over its local socket. Stopping it is one command, `crystalline ctl shutdown`, and the next connection simply spawns a fresh one.

A daemon can also end up in a third state: alive and holding the index lock, but answering nothing on its socket. Nobody can attach to it and nobody can take the index from it, so it would block every client until the process died on its own. The next client that connects ends that state instead of waiting it out. It probes the socket for two seconds, and only when nothing answers and the service record names a process that is both alive and identifiably a Crystalline binary does it ask that process to stop, force it if it will not, clean up the record and start a fresh daemon. Anything less than certain about who holds the lock is refused rather than signalled, with an error naming the lock file. `crystalline status` reports the same state read-only as `unresponsive`, and `crystalline doctor --fix` performs the replacement on demand.

Upgrades need no manual restart: attaching is version aware, so the first client built from a newer binary (after a `brew upgrade`, say) asks the older daemon to shut down gracefully and a new daemon starts on the current version. The takeover is one-way - an older client attaches to a newer daemon rather than displacing it back. Agent sessions ride through the swap, because the stdio bridge reconnects when its daemon goes away, replays the MCP handshake and answers any request the restart orphaned with a retryable error instead of silence.

## Claude Desktop extension

For someone who never opens a terminal, the `.mcpb` bundle wraps the same binary in a one-click Claude Desktop extension. The universal bundle (`crystalline-v<version>.mcpb`) covers Apple Silicon Macs and Windows (windows-arm64 runs it via x64 emulation) in one download, and is what the Connectors Directory listing carries; four per-arch bundles remain for intel Macs, native windows-arm64 and anyone who would rather have the smaller download for their exact platform. Whichever one is installed, it takes no configuration and starts with no domains: under the hood Claude Desktop spawns `crystalline mcp` over stdio, landing on the same daemon as the personal workstation shape. That daemon opens the web UI at `http://localhost:7411` by default, so installing the extension is also how a Desktop user gets the browser half: the first visit creates the admin account in the browser and everything the agent captured is a page to read (`service.http: false`, or `CRYSTALLINE_SERVICE_HTTP=false`, turns the endpoint off). The agent creates a domain whenever it needs somewhere to capture knowledge, with the `add_domain` tool - a folder of markdown files under `Documents/Crystalline` (the default domains root, overridable with `domains_root` or `CRYSTALLINE_DOMAINS_ROOT`), a database-backed virtual domain or a GitHub team domain. Onboarding is automatic - the server's instructions deliver the routing block on every connection, empty at first - and the release's crystalline-claude-desktop-skill zip adds capture and collaboration best practices as an uploadable skill (see [Skills](../README.md#skills) in the README).

```mermaid
flowchart LR
    CD[Claude Desktop] -->|stdio, one-click install| D[Daemon]
    A[Agent] -->|add_domain| D
    B[Browser] -->|HTTP :7411| D
    D --> K[Domains under Documents/Crystalline]
    D --> I[Index]
```

Sometimes the bundled binary can neither reach the daemon nor take the index lock, typically because the extension is older than a Crystalline installed another way (a Homebrew install after a `brew upgrade`, say). The session still connects instead of failing: the server comes up degraded with a single `status` tool and instructions that tell the model (and through it the user) to download the latest extension from the GitHub releases page and install it over the current one. Claude Desktop has no in-app update mechanism for an extension, so installing the downloaded `.mcpb` over the existing one is the update path.

## Team server

For a team, run the GHCR image (see [Run in a container](#run-in-a-container)) via `examples/docker/compose.yaml`: the daemon listens on `--http 0.0.0.0:7411` and every agent on the network reaches it over streamable HTTP instead of stdio. Knowledge is bind-mounted from the host so it stays exactly the same markdown files, a `/data` volume holds the disposable index, and the slim `latest` image downloads the model into that same volume once (pick `with-model` instead to skip the download). Agents that reach the daemon by a hostname rather than `localhost` need that host in `CRYSTALLINE_SERVICE_ALLOWED_HOSTS` (the transport validates the `Host` header; see [Configure through environment variables](#configure-through-environment-variables)). The MCP transport caps a request body at 10 MiB, the same limit the JSON API applies to everything but its two archive-upload routes, so an engram that saves from the browser also writes through an agent's `write_engram` and a bigger one is refused on both with `413 Payload Too Large` before any tool runs; an operator meets this through a proxy first, which is why anything in front of the port needs its own body limit raised to match (see [Team server with Fluid](#team-server-with-fluid)). See [Run in a container](#run-in-a-container) for the compose file and image tags.

```mermaid
flowchart LR
    A1[Agent] -->|HTTP :7411| D[Daemon]
    A2[Agent] -->|HTTP :7411| D
    D -->|bind mount| K[Knowledge files]
    D -->|volume| I[Index]
```

## Web UI from the daemon

The daemon serves Fluid itself, and it does so by default. The browser UI is built into the binary and the HTTP endpoint opens at `127.0.0.1:7411` unless something turns it off, so one process on one port answers both the agents that speak MCP and the people who want to read what those agents learned: point a browser at `http://localhost:7411` and the app is there, with no second container, no static bundle to deploy and no version to keep in step. `serve --http <host:port>` moves the endpoint somewhere else and `serve --http off` (or `service.http: false`, or `CRYSTALLINE_SERVICE_HTTP=false`) closes it. It is the same UI the compose variant below runs - domains down the side, an engram with its frontmatter, observations, relations, backlinks and neighborhood graph, faceted search and Cmd+K to jump anywhere - served straight from the daemon that holds the index.

```mermaid
flowchart LR
    B[Browser] -->|HTTP :7411| D[Daemon]
    A[Agent] -->|MCP over HTTP :7411| D
    D --> K[Knowledge files]
    D --> I[Index]
```

The web UI and the JSON API are both on by default wherever the HTTP endpoint is on, so there is nothing to enable. Two settings turn them off: `service.ui: false` (or `CRYSTALLINE_SERVICE_UI=false`) serves the API and MCP without the UI, which is the shape a separate Fluid deployment fronts, and `service.api: false` leaves MCP and `GET /health` alone on the port and takes the UI down with it, since a UI without its API is a shell that can only render a login error. Both are read once when the HTTP surface starts, like `service.read_only`, so changing one needs a daemon restart, and the line the daemon prints on start names which of the two is off.

Accounts work exactly as in [Team server with Fluid](#team-server-with-fluid), because it is the same API behind the same UI: nothing is readable until an account exists. The browser is where that first account is normally made. A daemon that has no accounts yet greets the first visit with a create-first-admin form instead of a login form: fill in a name and a password, and you are that instance's admin and signed in already. There is no second chance for a passer-by, because once any account exists the endpoint behind that form is gone for good and answers `410 Gone` to everyone, forever. `crystalline users add ada --role admin` does the same job from a terminal (it prompts for a password, or `--password-stdin` reads it from a pipe) and stays the scripted and recovery path; it writes the accounts database directly, so a running daemon picks the new account up on its next lookup with nothing to restart. One platform is the exception: on Windows the accounts database cannot be shared between two processes (the database engine has no cross-process coordination there yet), so `crystalline users` has to wait until the daemon is stopped, and the browser is the way to manage accounts while it serves. The static shell is served to anyone who connects, exactly as nginx serves it in the compose variant; every byte of knowledge still comes through the JSON API, which answers `401` to a request that carries no identity.

Who may fill that form in is decided by the connection itself, never by anything a request can claim. A caller on the machine that serves the instance is trusted on the strength of its loopback peer address and is asked for nothing more. A bind anyone else can reach cannot be, so a `serve` that binds anything but loopback (`0.0.0.0`, a LAN address, the container default) mints a one-time setup token for that process and prints it as it starts, on the same startup banner as the rest of its startup lines. Read it wherever that banner goes: the terminal for a `serve` you typed, `docker logs` for a container and `journalctl -u crystalline` under systemd (both of those run `serve` in the foreground and collect what it prints), or the daemon log for a daemon running in the background. The form asks for the token only when the server says it is required, so a loopback user never sees the field, and a remote one pastes in the printed value. The token belongs to that one serve process: restart the daemon and the next one prints a fresh token.

The same rule has a caveat worth knowing before a proxy goes in front. A request carrying `X-Forwarded-For`, `Forwarded` or the other forwarding headers is never counted as local, even when the proxy that sent it runs on this very machine, since a loopback peer with proxy headers is a reverse proxy relaying somebody remote. So the wizard behind a proxy asks for a token, and a daemon bound to loopback has none to give. Create the first admin before you put a proxy in front of the daemon, or bind a network address so that `serve` prints a token to paste.

The `Host` allow-list guards the MCP endpoint only, so a browser needs no entry in it: open `http://server.lan:7411` and the UI loads and calls the API with nothing configured. An agent that reaches MCP by that same name still needs `server.lan` in `CRYSTALLINE_SERVICE_ALLOWED_HOSTS`, as it always did (see [Configure through environment variables](#configure-through-environment-variables)).

Serve it over TLS anywhere but localhost. The session cookie is marked `Secure` for any browser `Host` that is not loopback, so a browser on a plain `http://team.lan` is handed a cookie it refuses to store and signing in never sticks. Put a TLS terminator in front of the daemon; it has to forward the `Host` header verbatim, leave `Origin` alone, pass WebSocket upgrades through for `/api/v1/collab/` or the editor runs solo-only, and accept a request body at least as large as the daemon does - an nginx terminator here carries the same 1 MiB default as the compose config and needs the same `client_max_body_size` raised to match, 10 MiB for the API in general and 64 MiB for the two archive paths (`/api/v1/domains/*/archive/preview` and `.../import`), which carry a whole domain rather than one document. [Team server with Fluid](#team-server-with-fluid) spells out the full proxy requirements, and they are the same here.

Read-only and anonymous serving need no extra parts either. `--read-only` refuses every write inside the API while the UI, which is GETs, is served in full and renders itself read-only from its own capability probe; `auth.anonymous: true` answers identityless requests at viewer level. The two together are the published archive anyone who can reach the port may browse without signing in at all, with no nginx anywhere in it (see [Read-only deployments](#read-only-deployments)).

A binary built without the bundle says so rather than pretending: the startup line reads `web UI not built into this binary`, and a navigation gets a `503` page naming `pnpm --dir fluid build` while the JSON API and MCP carry on unaffected. That is the state of a clone built without node, or of a `--no-default-features` build; every release binary carries the bundle, and so does the container image, which repackages the same binary.

## Team server with Fluid

The scale-out variant. The daemon serves the UI itself now ([Web UI from the daemon](#web-ui-from-the-daemon)), so reach for this shape when the front door deserves a tier of its own: nginx replicas that scale independently of the daemon, gzip on the wire, one obvious place to terminate TLS, and an upgrade decoupling where the UI image can move ahead of the daemon (which is why Fluid tells a browser when the two versions have drifted apart). It is the team server with a browser tier in front of it. `deploy/fluid/docker-compose.yml` runs two containers on one network: the same Crystalline daemon as above, and Fluid, the web UI, as an nginx image (`ghcr.io/jordiboehme/crystalline-fluid`) that serves a static bundle on port 80 and forwards `/api/` to the daemon. Agents keep reading and writing over MCP exactly as before; people get the same knowledge as pages they can browse, search, link to and read side by side with what an agent was taught. Crystalline stores what was learned; Fluid is where you think with it.

```mermaid
flowchart LR
    B[Browser] -->|HTTP :80| F[Fluid, nginx]
    F --> S[Static bundle]
    F -->|/api to crystalline:7411| D[Daemon]
    A[Agent] -->|MCP over HTTP| D
    D -->|bind mount| K[Knowledge files]
    D -->|volume| I[Index]
```

The bundled nginx config carries its own dedicated location for the collab WebSocket, `/api/v1/collab/`, with the Upgrade and Connection headers the plain `/api/` block does not send. Anyone who replaces the Fluid image's nginx config with their own has to carry that block over, or the upgrade never reaches the daemon and the editor runs solo-only with nothing on screen to explain why. The config also raises `client_max_body_size` to 10 MiB to match the daemon's own limit, and a replacement config that leaves nginx's 1 MiB default in place turns every save and every archive import of a larger engram into a proxy refusal. The archive routes get a second, larger directive of their own, in a regex location matching `^/api/v1/domains/[^/]+/archive`: a domain export is re-imported as one zip with its `assets/` attachments inside it, so those two paths accept 64 MiB where everything else keeps 10 MiB, exactly as the daemon does. A replacement config that carries the server-level directive over but not that location refuses an archive the same deployment produced, which reads on screen like a Fluid bug rather than a proxy one.

Nothing is readable until an account exists: the JSON API answers `401` to a request that carries no identity. Creating the first admin is the one bootstrap step, and there are two ways to take it. From the host, `users add` writes the accounts database directly (never through the daemon), so a running instance picks the new account up on its next lookup with nothing to restart:

```sh
docker compose -f deploy/fluid/docker-compose.yml up -d

printf '%s' 'the-password' | docker compose -f deploy/fluid/docker-compose.yml \
  exec -T crystalline crystalline users add ada --role admin --password-stdin
```

Or take it in the browser instead: open Fluid and an instance with no accounts renders the create-first-admin form ([Web UI from the daemon](#web-ui-from-the-daemon)). The daemon binds `0.0.0.0` inside its container, so that form asks for the one-time setup token the daemon printed when it started, and `docker compose -f deploy/fluid/docker-compose.yml logs crystalline` is where to read it (a restarted daemon prints a new one). Either route mints the same admin, and either one closes the form for good.

Restart the daemon after upgrading the binary, before editing accounts, so both sides open the accounts database the same way. A container upgrade recreates the container and has this covered already; a native install upgraded underneath a daemon that keeps running does not.

`-T` because `--password-stdin` reads a pipe and compose would otherwise allocate a terminal there is nothing to read from. Everyone else is `--role viewer` or `--role editor`. Two other identity modes need no accounts at all: `CRYSTALLINE_AUTH_ANONYMOUS=true` serves an identityless request at viewer level, which together with `--read-only` is a published archive anyone who can reach it may browse, and `CRYSTALLINE_AUTH_TRUSTED_HEADER=remote-user` takes the already-authenticated user from a header an SSO proxy sets, creating that account at viewer role the first time it is seen. The trusted header is only safe when the proxy sets it itself and strips whatever a client sent, which means that proxy has to sit in front of Fluid rather than behind it: nginx forwards client headers untouched. A proxy that speaks the standard forward-auth quartet has `CRYSTALLINE_AUTH_PROXY_HEADERS=true` instead, on the same terms and with the same warning: see [Proxy forward auth](#proxy-forward-auth). Provisioning is capped by `auth.max_users`, default 100; existing accounts always resolve, and `crystalline users` is never capped.

Those three modes are about the people using Fluid. The agents connecting over HTTP MCP are governed separately, by `auth.mcp`, which is its own scenario with its own token-issuance story and its own consequences for what an authenticated agent may see: see [Authenticated agents](#authenticated-agents).

An admin can also manage domains from Fluid itself: create local, virtual or GitHub team domains, connect GitHub under Settings, and download or import a domain as a zip archive. Download archive and Import archive on the domain page are the plain backup and restore story - the zip holds the domain's markdown files (MANIFEST included) and imports through the normal write path, so the index stays in sync.

Serve it over TLS anywhere but localhost. The session cookie is marked `Secure` for any browser `Host` that is not loopback, and for any request a proxy in front reports as `https`, so a browser on a plain `http://team.lan` is handed a cookie it refuses to store and signing in never sticks. Put a TLS terminator in front of the Fluid container and the same deployment works unchanged: Fluid passes an existing `X-Forwarded-Proto` through rather than overwriting it with its own hop.

Any proxy in front of the daemon's HTTP endpoint needs WebSocket upgrade pass-through for `/api/v1/collab/` as well as plain HTTP forwarding: nginx needs `proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade";` (the bundled `nginx.conf.template` carries the full block). The Origin header must reach the daemon unmodified, and the Host header must stay the browser-facing host rather than being rewritten to an internal name - the server's same-host Origin check compares the two, and a proxy that breaks either the upgrade or that comparison gets a `403 Forbidden` whose detail names Origin, not the proxy, which is the tell for diagnosing it. Forward the Host header verbatim: in nginx that is `proxy_set_header Host $http_host;` and not `$host`, which drops the port, so a deployment published on any port but the default answers every join with that 403 while the Origin the browser sent carries `:8080`.

Fluid holds no state of its own, so it scales to as many replicas as a deployment wants. One variable configures the image: `CRYSTALLINE_UPSTREAM`, the daemon's `host:port`, `crystalline:7411` by default. nginx resolves it once, when it loads its configuration, which is why the compose file gates Fluid on the daemon's healthcheck and why a daemon that moves to a new address needs the Fluid container restarted rather than only itself. Reading is what the UI is for: the sidebar lists what this instance knows about, an engram page carries its frontmatter, observations, relations, backlinks and an interactive neighborhood graph, and Cmd+K (Ctrl+K where there is no Cmd key) opens a command palette that jumps to any domain, or to any engram by title, from anywhere in the app.

## Linux server with systemd

The team server shape without a container: the `.deb` ships a systemd unit,
installed disabled, so installing the package never starts anything. Put any
overrides in `/etc/default/crystalline` (bind address, read-only mode, team
domains - the same variables as [Configure through environment
variables](#configure-through-environment-variables)) and turn the service on:

```sh
sudo systemctl enable --now crystalline
```

The unit runs `crystalline serve` in the foreground under a dynamic service
user: the index and socket live in `/var/lib/crystalline`, the model cache in
`/var/cache/crystalline` and the config in `/etc/crystalline/config.yaml`.
HTTP is on at `127.0.0.1:7411` by default, so the unit serves MCP, the JSON
API and the web UI there as installed, and `CRYSTALLINE_SERVICE_HTTP=false`
in `/etc/default/crystalline` closes the endpoint (the file is read after the
unit's own defaults, so a line there wins). Set
`CRYSTALLINE_SERVICE_HTTP=0.0.0.0:7411` there instead to let agents on the
network learn from it, and probe `GET /health` from a load balancer or uptime
monitor without an MCP handshake. That network bind is also the one that makes `serve` print a
one-time setup token for the browser's create-first-admin form ([Web UI from
the daemon](#web-ui-from-the-daemon)); the service user has no terminal, so
read it back with `journalctl -u crystalline`. The sandbox makes the
filesystem read-only outside those directories, so grant each knowledge
folder a write allowance with a drop-in: `sudo systemctl edit crystalline`,
then `ReadWritePaths=/srv/knowledge` under `[Service]`. Check on the daemon
with `systemctl status crystalline` and `journalctl -u crystalline` rather
than `crystalline ctl` (the daemon's socket lives under the service user, out
of reach of a login shell). A tarball install gets the same unit from the
repository at `crates/cli/debian/crystalline.service`, copied to
`/etc/systemd/system/` with ExecStart adjusted to where the binary landed (a
standalone binary is often `/usr/local/bin/crystalline`). Upgrading the
package restarts the service only if it is running; a disabled unit stays
untouched.

```mermaid
flowchart LR
    M[Uptime monitor] -->|GET /health| D[Daemon under systemd]
    A1[Agent] -->|HTTP :7411| D
    A2[Agent] -->|HTTP :7411| D
    D --> K[Knowledge folders]
    D --> I[Index in /var/lib/crystalline]
```

## Published read-only domains

When a team curates knowledge as a reviewed git repository instead of writing into the container directly, `examples/docker/compose.git-sync.yaml` adds a sidecar that pulls the repository into a shared volume every 60 seconds and mounts it read-only into Crystalline. The daemon runs with `--read-only`, so the write-gated tools disappear from the MCP tool list (the seven of them are named under [Read-only deployments](#read-only-deployments)) and agents can only search and read, while sync, the file watcher and embedding keep following every pull. A team domain connected to a GitHub origin (see [Team knowledge on GitHub](#team-knowledge-on-github)) gets the same effect natively, no sidecar container needed: `update_domain` and `origin_status` stay visible even in read-only mode once `github.enabled` is on (with it off no collaboration tool is listed at all), so a read-only instance keeps a team domain current on its own background poll schedule. A third option needs no mounted config and no sidecar at all: an immutable image started with `CRYSTALLINE_SERVICE_READ_ONLY=true`, `CRYSTALLINE_GITHUB_ENABLED=true`, one or more `CRYSTALLINE_DOMAIN_<NAME>` and `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` pairs and `CRYSTALLINE_GITHUB_TOKEN` for a headless sign-in bootstraps each team domain itself on first start and keeps it current on the same background poll schedule, with nothing left to mount or edit ever again. See [Read-only deployments](#read-only-deployments) and [Configure through environment variables](#configure-through-environment-variables) for the full behavior and variable list.

```mermaid
flowchart LR
    G[Git repository] -->|pull every 60s| S[Sidecar]
    S --> K[Knowledge files]
    K -->|read only| D[Daemon]
    D -->|HTTP, read and search only| A[Agent]
```

## Air-gapped or egress-restricted

When a host has no outbound network access, or the first-start model download delay is unwanted for any other reason, use the `with-model` image, or set `CRYSTALLINE_MODELS_DIR` on any install to point at a model directory fetched ahead of time, so nothing in the runtime path ever needs the network. This is orthogonal to read access: combine it with either the read-write [team server](#team-server) shape or the read-only git-sync shape, since air-gapping is about the model rather than about who can write. See [Run in a container](#run-in-a-container) for the image variants and `CRYSTALLINE_MODELS_DIR`.

```mermaid
flowchart LR
    M[Model] -->|baked into image| D[Daemon]
    D -->|HTTP, no egress| A[Agent]
```

## Shared database collaboration

When several instances should share one index instead of each keeping its own, point them at a shared PostgreSQL database with pgvector using `examples/docker/compose.postgres.yaml`: an immutable image with `CRYSTALLINE_DATABASE_BACKEND=postgres` and `CRYSTALLINE_DATABASE_URL` set, no mounted config.yaml needed. Every instance searches and reads everything in the shared database, so knowledge one instance captures is immediately visible to the rest. Writes follow a single-writer-per-domain rule: each file domain has exactly one hosting instance that syncs and watches its files. Hosting is arbitrated by a host lock with a 30 second heartbeat and a 90 second stale takeover, so a second instance that tries to sync a domain it does not host is refused with the name of the current host and serves that domain read-from-database only. A virtual domain keeps its engrams in the database itself rather than on disk, so it is shared truth that any instance may write, guarded per engram by a compare-and-swap on the checksum so a stale edit is refused rather than silently clobbered. The local-first guarantees hold for a running daemon against a local database; a remote database trades some latency for the federation payoff.

```mermaid
flowchart LR
    A1[Agent] --> D1[Daemon A]
    A2[Agent] --> D2[Daemon B]
    D1 -->|hosts and syncs domain X| K[Knowledge files X]
    D1 --> PG[(PostgreSQL + pgvector)]
    D2 --> PG
    D2 -.->|reads X from DB, hosts virtual domain Y| PG
```

## Team knowledge on GitHub

For a team that keeps a domain in a GitHub repository instead of a shared filesystem or database, each repository (optionally a subfolder of one) becomes a team domain, with an origin recording which repository, subfolder and branch it tracks. Members connect once with a short code confirmed in a browser they are already signed into: no git, no SSH keys and no token to paste for someone who only knows the GitHub web UI. Crystalline shares new knowledge as a proposal the team reviews and merges on GitHub itself, and brings each team domain up to date automatically in the background once its proposals merge; a genuine disagreement between local and team knowledge surfaces as a conflict, settled locally. A fleet of worker or agent hosts can join the same team domain with no interactive connect step at all: three environment variables - `CRYSTALLINE_DOMAIN_<NAME>`, `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` and `CRYSTALLINE_GITHUB_TOKEN` - register the domain, attach its origin and supply this machine's GitHub identity, so a new node bootstraps the domain itself and starts polling for updates on first start. See [Share knowledge with a team](../README.md#share-knowledge-with-a-team) in the README for the full verb set and the one-time connect flow, and [Configure through environment variables](#configure-through-environment-variables) for the fleet variant.

```mermaid
flowchart LR
    A1[Agent] --> D[Daemon]
    A2[CLI] --> D
    D -->|propose| P[Pull request]
    P -->|reviewed and merged on GitHub| G[GitHub repository]
    G -.->|poller, auto-update| D
    D --> K[Team domain files]
```

## Run in a container

Crystalline publishes a multi-arch OCI image (`linux/amd64` and `linux/arm64`) to GHCR on every release, for Linux server deployments. macOS and Windows have no OCI container runtime worth targeting here, so those platforms run the native binary (see [Install the binary](../README.md#install-the-binary) in the README); the container covers the Linux server case.

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

That one published port carries the browser too: the image serves the web UI built in, so `open http://localhost:7411` lands on Fluid while agents keep speaking MCP to the same address (see [Web UI from the daemon](#web-ui-from-the-daemon) for accounts, TLS and the two toggles). A fresh container has no accounts, so that first visit renders the create-first-admin form. The container binds `0.0.0.0`, which is not loopback, so the form asks for the one-time setup token the daemon prints as it starts: `docker logs crystalline` is where to read it, and a restarted container prints a new one.

What persists where:

- `./knowledge` (bind mount) holds the engrams of every file domain, one subfolder per domain - the durable state for file-backed knowledge, exactly the same markdown-plus-frontmatter files the native binary reads.
- `crystalline-data` (named volume, mounted at `/data`) holds the search index and the embedding model cache. For file domains those two are fully rebuildable: losing them costs a `crystalline reindex --full` and a model re-download (skipped entirely on `with-model`, since its model lives outside `/data` and is never affected by the volume), never knowledge. Two things on this volume are not rebuildable, though. If you run virtual domains, their engrams are the source of truth and live here, so back this volume up or `crystalline domain export` them to the bind mount to keep a file copy, or download the domain as a zip from Fluid's domain page (admin). And `web-auth.db` holds the JSON API's accounts and sessions, which are data rather than derived state: losing it means creating every user again with `crystalline users add`.

Local domains created from Fluid land under the server's domains root, which defaults to `~/Documents/Crystalline` in the daemon's home; in a container set `CRYSTALLINE_DOMAINS_ROOT` to a persistent path (a bind mount, or a folder under the `/data` volume such as `/data/domains`) so UI-created domains survive the container.

The maintenance pending state lives in the daemon's state directory, so a containerized daemon paired with a host-side Stop hook never arms the pending ask: the two processes see different state directories, and the domains one records a human writing to are not the ones the other reads before it nudges. The hook still asks on its weekly arm, from its own clock on the host.

The image runs as the non-root user `65532:65532` and ships `/data` owned by it, so an empty named volume mounted there is writable from the first start: Docker copies the image directory's ownership into the volume when it initializes it. A bind mount never inherits that - the host directory keeps its own ownership - so a host folder mounted at `/data` instead of a named volume has to be made writable by that uid first, or the daemon cannot create its state directory and the container restarts in a loop:

```sh
mkdir -p ./crystalline-data
sudo chown 65532:65532 ./crystalline-data
```

The same applies to the bind-mounted knowledge folder whenever the daemon writes into it (an agent's `write_engram`, or the generated `index.md` files), which includes the case where `docker run` creates a missing bind-mount source itself: Docker creates it root-owned. A named volume put in that folder's place is no different, because the ownership Docker copies into a fresh volume is the mount point's in the image and the image ships no `/knowledge`: it comes up root-owned exactly like a bind-mount source Docker created, so give it to the daemon's uid once - `docker run --rm -v crystalline-knowledge:/knowledge alpine chown -R 65532:65532 /knowledge`, from an image that has a shell, since Crystalline's is distroless - before `domain init` writes anything into it. A knowledge folder mounted read-only, as in `compose.git-sync.yaml`, needs nothing.

The `with-model` variant sets `CRYSTALLINE_MODELS_DIR` (also settable directly, on any install, to relocate the model cache anywhere else) to a path outside `/data` so the baked model is never shadowed by the `/data` volume mount. The bundled model is [BAAI/bge-small-en-v1.5](https://huggingface.co/BAAI/bge-small-en-v1.5), MIT licensed.

Both variants ship a built-in Docker `HEALTHCHECK` that probes `GET /health` with no shell involved (the image is distroless), so `docker ps` reports health directly and a Compose service can gate on `condition: service_healthy`. External monitors (a Kubernetes `httpGet` probe, an uptime checker such as Gatus, a load balancer) can probe the same `/health` endpoint directly rather than going through Docker's own health state.

Sample Compose files ship under [`examples/docker/`](../examples/docker/), and the one deployment that is meant to be run rather than read from under [`deploy/`](../deploy/):

- **`compose.yaml`** - the single-container setup above, plus a commented one-shot `domain init` / `domain add` recipe for bootstrapping a fresh domain (`domain add` indexes it immediately, routed to the running daemon over the shared `/data` volume).
- **`compose.git-sync.yaml`** - a scale-deployment variant that adds a sidecar keeping the knowledge folder synced from a git remote every 60 seconds, mounted read-only into Crystalline. This is the pattern for a team that manages engrams as a reviewed git repository rather than writing into the container directly.
- **`compose.postgres.yaml`** - the [Shared database collaboration](#shared-database-collaboration) setup: a Postgres service with pgvector plus a Crystalline instance pointed at it via environment variables, and a commented second instance showing how a second worker shares the same database. Reach for it when several instances should share one federated index instead of each keeping its own.
- **`deploy/fluid/docker-compose.yml`** - the [Team server with Fluid](#team-server-with-fluid) setup: the daemon plus the browser UI on port 80, with the first-admin bootstrap in a comment. It can build the Fluid image locally (`docker compose build fluid`) as well as pull it.

### Configure through environment variables

An immutable image with no `config.yaml` to mount or edit configures purely through the environment: every settings key maps mechanically to `CRYSTALLINE_` plus the key uppercased with dots replaced by underscores (`github.enabled` becomes `CRYSTALLINE_GITHUB_ENABLED`), plus a handful of variables covering what has no settings-registry key of its own:

| Variable | Maps to | Notes |
|---|---|---|
| `CRYSTALLINE_SERVICE_READ_ONLY` | `service.read_only` | `serve --read-only` still forces it on |
| `CRYSTALLINE_SERVICE_HTTP` | `service.http` | the HTTP endpoint carrying MCP, the JSON API and the web UI: on at `127.0.0.1:7411` by default, `false` turns it off, `true` spells the default out, or give it a `host:port` to bind instead. `serve --http` wins over it, `serve --http off` closes it |
| `CRYSTALLINE_SERVICE_UI` | `service.ui` | `true` (default) serves the embedded Fluid web UI on the HTTP endpoint; `false` serves the JSON API and MCP only, the shape a separate Fluid deployment fronts. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_SERVICE_API` | `service.api` | `true` (default) serves the JSON API under `/api/v1`; `false` leaves MCP and `GET /health` alone on the port and disables the web UI with it, since a UI without its API is a shell that can only render a login error. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_SERVICE_ALLOWED_HOSTS` | `service.allowed_hosts` | comma-separated `Host` allow-list; loopback is always allowed and a single `*` allows any Host; `serve --allowed-host` wins over it |
| `CRYSTALLINE_SERVICE_RESPONSE_FORMAT` | `service.response_format` | `toon` (token-efficient list results, default) or `json` |
| `CRYSTALLINE_SKILLS_SERVE` | `skills.serve` | `auto` (default), `true` or `false`. Governs the shipped agent skills over MCP: the `skills` tool, `skill://` resources and the onboarding and connector prompts. `auto` serves them to every client except a stdio session spawned by a harness this machine has already onboarded, which has the skills as files and is onboarded by its own hook; such a session also gets a one-line pointer instead of the full instructions block. That is resolved by the spawned `crystalline mcp` process, from the `--harness <name>` argument `crystalline install` writes into the harness's MCP registration plus the local install receipt, never from anything the connecting client says. `true` always serves them, `false` never does. Read once when the daemon starts, like `service.read_only`, so a `configure` write applies at the next start. See [The stdio and HTTP surfaces can differ](#the-stdio-and-http-surfaces-can-differ) |
| `CRYSTALLINE_DATABASE_BACKEND` | `database.backend` | `turso` or `postgres` |
| `CRYSTALLINE_DATABASE_URL` | `database.url` | may carry a password, so it is always treated as a secret: `config show` and the `configure` tool render it as `(set)` whatever it holds, an embedded-backend file path included, and `crystalline doctor` reports the variable as set without its value |
| `CRYSTALLINE_GITHUB_ENABLED` and the other `github.*` keys | `github.enabled`, `github.poll_secs`, `github.api_url`, `github.oauth_client_id` | `github.stacks`, `github.share_identity` and `github.agent_identity` are `github.*` keys too and have their own rows below |
| `CRYSTALLINE_GITHUB_STACKS` | `github.stacks` | `true` (default) lets a share stack a new proposal on the domain's open one where the forge serves stacked pull requests, so each share gets its own focused review and reviewers merge the chain bottom-up; `false` keeps one proposal per domain, updated in place |
| `CRYSTALLINE_GITHUB_SHARE_IDENTITY` | `github.share_identity` | `instance` (default) credentials everything with the one connected instance token, the behavior an install has always had; `personal` credentials every origin write with the acting person's own connected GitHub identity, while pulling stays on the instance token. A value this build does not know reads as `instance`, so a hand-edited typo never flips the mode |
| `CRYSTALLINE_GITHUB_AGENT_IDENTITY` | `github.agent_identity` | the Crystalline account whose connected GitHub identity unauthenticated agent shares over HTTP MCP run under in personal mode - typically a bot account an admin connected. Consulted only where agents are not made to authenticate: with `auth.mcp` on, an agent shares as the account it authenticated as. Unset (default) means those shares are refused rather than falling back to the instance token; an empty value is no override, so clearing a configured name happens through `configure` or `config set`. Lowercase letters, digits, `.`, `_` and `-` only |
| `CRYSTALLINE_SEARCH_SALIENCE_WEIGHT` | `search.salience_weight` | 0.0 to 1.0 (default 0.15); how strongly a salient engram is lifted in hybrid ranking |
| `CRYSTALLINE_SEARCH_RETIRED_WEIGHT` | `search.retired_weight` | 0.0 to 1.0 (default 0.6, 1.0 disables); the ranking multiplier for deprecated, superseded, archived or legacy engrams |
| `CRYSTALLINE_IDENTITY_ACTOR` | `identity.actor` | who is recorded as the writer of an engram (`generated.by`), for example `team-bot/1.0` or `human:jordi`; unset means the connected MCP client identifies itself |
| `CRYSTALLINE_INDEX_FILES` | `index.files` | `true` (default) keeps a generated `index.md` in every folder of a file domain |
| `CRYSTALLINE_CAPTURE_SIMILAR` | `capture.similar` | attach the nearest existing engrams to every `write_engram` and content `edit_engram` receipt as a `similar` list with guidance to merge, supersede, link or ignore them (default `true`); `false` switches the advisory off everywhere, MCP and Fluid alike |
| `CRYSTALLINE_AUTH_TRUSTED_HEADER` | `auth.trusted_header` | the request header a trusted reverse proxy sets to name the already-authenticated user, for example `remote-user`. Unset (default) means no header is believed, whatever a client sends; an account named by a configured header is created at viewer role the first time it is seen. Only safe when the proxy in front of Crystalline strips the header from client requests and sets it itself. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_PROXY_HEADERS` | `auth.proxy_headers` | `true` trusts the standard forward-auth quartet a reverse proxy sets - `Remote-User` names the person, `Remote-Name` and `Remote-Email` say how to show them, `Remote-Groups` is parsed and ignored until claim mapping exists. `false` (default) believes none of them, whatever a client sends. **Trust-the-proxy: only safe when Crystalline is unreachable except through that proxy and the proxy strips client-supplied copies of those headers.** Mutually exclusive with `CRYSTALLINE_AUTH_TRUSTED_HEADER`; setting both leaves the HTTP endpoint refusing to start (a warning names both keys and says to pick one) while the daemon keeps running and MCP over stdio is unaffected. See [Proxy forward auth](#proxy-forward-auth). Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_ANONYMOUS` | `auth.anonymous` | `true` serves JSON API requests that carry no identity at all, at viewer level; `false` (default) answers them `401`. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_MCP` | `auth.mcp` | `true` requires every MCP connection over HTTP to authenticate with a personal MCP token: a request without a valid `Authorization: Bearer cmt_...` header is answered `401` with `WWW-Authenticate: Bearer` before the transport sees it, so no session ever opens. A signed-in user issues a token in Fluid under profile > Agent access; an admin issues one for any account from the CLI. `false` (default) is the legacy open tier, where any client that can reach the port is served. Local stdio clients are unaffected: those are the machine owner. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OAUTH` | `auth.oauth` | Serve OAuth for MCP clients: the well-known metadata, dynamic client registration, authorization with a consent page and a token endpoint, so a hosted client such as Claude.ai connects without a pasted token. Unset (default) follows `auth.mcp`: on where every agent authenticates and the UI is served, off otherwise. `false` turns it off outright. `true` insists, and needs `auth.mcp`, since the tokens it issues are checked at that gate, `service.api`, since its endpoints live under `/api/v1`, and `service.ui`, since the authorization leg redirects the browser to the consent page at `/authorize`, a Fluid route - with any of the three off the daemon refuses to start. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_MAX_USERS` | `auth.max_users` | how many accounts external provisioning may mint in total - the trusted header, the forward-auth headers and single sign-on all count against it (default 100). Only minting a *new* account is refused past the cap; an existing account always resolves, and `crystalline users` is never capped. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_ISSUER` | `auth.oidc.issuer` | the single sign-on provider's issuer url, the one discovery appends `/.well-known/openid-configuration` to. Unset (default) means SSO is off and only the local accounts sign in: single sign-on is opt-in on top of them, never a replacement. On Entra this is the tenant-specific url `https://login.microsoftonline.com/<your-tenant-id>/v2.0`; the tenant-independent `{tenantid}` template a portal page shows, and the `common`, `organizations` and `consumers` endpoints, are all refused with a correction naming the fix (see [Enterprise SSO](#enterprise-sso)'s Entra notes). Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_CLIENT_ID` | `auth.oidc.client_id` | the client id (application id) the provider issued for this Crystalline instance. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET` | `auth.oidc.client_secret` | the client secret that goes with the client id. A credential, and it is never echoed back: `crystalline config show` and the `configure` tool render it as `(set)`, and `crystalline doctor` reports the variable as set without its value. Setting it here is how a deployment keeps the secret out of `config.yaml` entirely; the variable wins over a value in the file either way. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_NAME` | `auth.oidc.name` | the provider's display name, the label on the sign-in button. Unset means the generic wording. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_SCOPES` | `auth.oidc.scopes` | the scopes requested at authorization, space separated. Unset means the standard set. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_DEFAULT_ROLE` | `auth.oidc.default_role` | the role an account provisioned through single sign-on is created at: `viewer`, `editor` or `admin`. Unset means `viewer`, the least privileged one. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_AUTH_OIDC_REDIRECT_URI` | `auth.oidc.redirect_uri` | the address the provider sends the browser back to, used verbatim instead of the one derived from each request's `Host` and forwarded scheme. An absolute `https` url (`http` only on loopback) ending in `/api/v1/auth/oidc/callback`, with no query and no fragment, spelled exactly as it is registered with the provider. Unset (default) derives it per request, which is right wherever the browser reaches this instance at the address the request says it does; set it where a proxy rewrites the `Host` and cannot be made to forward the public one. A value that cannot work leaves single sign-on off with a startup warning naming the key, rather than failing every sign-in at the provider's error page. **With `auth.oauth` on this key does a second job**: its origin (scheme, host and port, the callback path dropped) is the OAuth resource identifier, so it is what both well-known documents publish as `resource` and `issuer` and the audience every access token is minted for and checked against; unset, that identifier is derived from each request's `Host` by the same rule, and a value that cannot be parsed as an absolute `http` or `https` url falls back to that derivation with a startup warning. Read once when the HTTP surface starts, like `service.read_only` |
| `CRYSTALLINE_DOMAINS_ROOT` | `domains_root` | the default parent folder for a local domain created without an explicit path (by `add_domain`, the CLI or Fluid); defaults to `~/Documents/Crystalline` in the daemon's home |
| `CRYSTALLINE_UPSTREAM` | n/a (Fluid image only) | not a Crystalline setting: it is read by the Fluid image, and names where its nginx forwards `/api/`, as `host:port` (default `crystalline:7411`). Read once, when nginx loads its configuration, so an upstream that changes address needs the Fluid container restarted. See [Team server with Fluid](#team-server-with-fluid) |
| `CRYSTALLINE_CONFIG` | an alternate config file path | `--config` wins over it |
| `CRYSTALLINE_DOMAIN_<NAME>` | a domain rooted at that path, overlay only | never written to `config.yaml` |
| `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` | `owner/repo[/subpath][@branch]` | bootstraps the domain on first start |
| `CRYSTALLINE_GITHUB_TOKEN` | this machine's GitHub token | read-only; `connect github` refuses while set |
| `CRYSTALLINE_MODELS_DIR` | the model cache path | pre-existing, unchanged |
| `CRYSTALLINE_CHANNEL` | install channel marker | set to `mcpb` by the Claude Desktop extension manifest so degraded-startup copy tells the user to update the extension rather than the binary; not meant to be set by hand |

`<NAME>` in a domain variable is lowercased with underscores turned into hyphens for the domain name itself (`CRYSTALLINE_DOMAIN_TEAM_KNOWLEDGE` becomes the domain `team-knowledge`). Precedence, highest first: a command-line flag, then an environment variable, then `config.yaml`, then the built-in default; an environment value is never written back to the config file.

## Private domains

Every domain starts shared: every account on the instance may read it, following the roles [Team server with Fluid](#team-server-with-fluid) describes. A domain may instead be made private - visible to its owner, the accounts invited into it and instance admins, and to nobody else. A private domain a caller cannot see answers exactly as a domain nobody registered: absent from the domain list, absent from search, "not registered" when named directly. There is no way to tell "private" apart from "does not exist" from the outside, which is the point.

**Membership levels**, from least to most: viewer (read and search, nothing more), editor (everything a viewer may do, plus writing and editing its engrams), manager (everything an editor may do, plus inviting, removing and re-leveling members, including other managers). The owner sits above all three: only the owner of a private domain, or an instance admin, changes its visibility, transfers it or removes it, so a manager may fill a domain with people but cannot re-share it, hand it to someone else or delete it. A shared domain has no owner, so only an instance admin removes one; privatizing an existing shared domain is admin-only too, even for the account about to own it.

**Instance admins always see and administer every domain**, private ones included - the same rule a GitHub organization owner already knows about its repositories. And whoever holds the machine administers every domain too, invited or not: the CLI runs on the host that already holds the domain's files and the accounts database beside them, so a permission check there would protect nothing that is not already open to whoever is sitting at that terminal. Private domains protect an account from other accounts, never from whoever operates the machine.

Manage membership from Fluid's Members card on the domain, or from the CLI:

- **`crystalline domain members <domain> list`** - who owns the domain and who is invited, with each member's level.
- **`crystalline domain members <domain> add <user> [--level viewer|editor|manager]`** - invite an account, or move one to a different level. Defaults to viewer.
- **`crystalline domain members <domain> remove <user>`** - end a membership. The owner holds no membership row of its own; hand the domain to someone else with `domain transfer` instead.
- **`crystalline domain visibility <domain> private --owner <account>`** - close a shared domain, naming who owns it from here.
- **`crystalline domain visibility <domain> default`** - share it with every account again; this forgets who was invited.
- **`crystalline domain transfer <domain> <new-owner>`** - hand a private domain to a different account. The old owner keeps nothing; invite them back if they should stay.
- **`crystalline domain add <name> <path> --private --owner <account>`** - register a domain already private, owned by the account named, in one step.

Every one of these writes the accounts database directly from the host, the same way `crystalline users` does, so each works whether or not a daemon is running.

## Authenticated agents

`auth.mcp` (or `CRYSTALLINE_AUTH_MCP=true`) decides whether an HTTP MCP connection has to authenticate, separately from the three modes above that govern people in a browser. Off (the default) is the legacy open tier: any client that can reach the port is served, with no account behind the connection. On, every one of them presents a personal MCP token exactly like a human user, as `Authorization: Bearer cmt_...`, and a request without a valid one is answered `401` with `WWW-Authenticate: Bearer` before the transport sees it, so the session never opens. Every rejected credential gets the identical answer, so nothing about the response says which token exists or why it failed. Local stdio clients are unaffected either way, since those are the machine's owner; turn this on for any instance reachable beyond a trusted network. A hosted client that speaks OAuth needs no token pasted at all: see [MCP clients over OAuth](#mcp-clients-over-oauth).

**Issuing a token.** A signed-in user issues one for themselves in Fluid, under profile > Agent access. From the host that holds the accounts database, an admin issues one for any account instead: `crystalline users mcp-token <name>` (`--label <text>` names what it is for; `--list` shows the tokens an account already holds without ever printing one back, `--rotate <id>` replaces one keeping its label, `--revoke <id>` stops one at once). Either way the token is shown exactly once, at issuance - only its hash is stored, so a lost one is revoked and replaced rather than looked up. Both routes stay open on a read-only instance, which is the one place this API serves a write under `service.read_only`: a token is account state rather than knowledge, and an agent there cannot connect at all without one.

**Registering a harness against it.** However a harness spells a custom header for a remote MCP server, name `Authorization` carrying the printed token:

```json
{
  "mcpServers": {
    "crystalline": {
      "type": "http",
      "url": "https://team.example.com/",
      "headers": {
        "Authorization": "Bearer cmt_xxxxxxxxxxxxxxxxxxxxxxxxxxxx"
      }
    }
  }
}
```

That is the shape most remote-server-capable MCP clients use for a custom header; consult the harness's own docs for its exact spelling. Each request carries the header, not just the handshake: a session belongs to the account that opened it, and another account naming it - even with a valid token of its own - is answered `403` without being told whose session it is.

**What an authenticated agent may see.** It acts as its account: it reads the domains that account may read, its writes are refused where that account is only a viewer - of the instance, or of a private domain it was invited into as one - and a private domain it is not a member of is answered exactly as a domain nobody registered: absent from `list_domains`, absent from search, "not registered" when named (see [Private domains](#private-domains)). With `auth.mcp` off there is no account to be, so every agent on the open tier is the anonymous tier: it reads and writes what is shared and sees no private domain at all. On an instance where nobody has made a domain private, both tiers behave exactly as they always did. Every write an authenticated agent makes carries who it acted as in the engram's own provenance, unless `identity.actor` is set: `generated.by` records `<client>-for-<account>` - the MCP client's own name and the account it authenticated as, joined by the word `for` (a client calling itself `claude-code/2.0`, authenticated as `ada`, lands `claude-code/2.0-for-ada`) - so a later reader can tell which person's session made a given write. `identity.actor`, where configured, overrides this and every other actor string with its own fixed value (see the `CRYSTALLINE_IDENTITY_ACTOR` row above).

**What an authenticated agent may change about the instance itself.** Registering a domain (`add_domain`), changing settings (`configure` with `set`, `unset` or a connect) and deciding what a domain installs into this machine's harnesses (`provision` with `allow`, `deny` or `apply`) are instance-state changes, and with `auth.mcp` on they need the instance admin role, exactly as the same actions do over the JSON API. An agent authenticated as a viewer or an editor is refused with a message naming the role and the way to ask for the change; reading the settings back with a bare `configure`, or the decisions back with `provision` `status`, is a read and stays open to everyone (a bare `configure` shows the issuer, the client id, the trusted-header name and the allowed hosts, with every secret masked; `provision` `status` names only domains the caller may see). Unregistering a domain (`remove_domain`) follows the private-domain rule instead of the admin one, since it is narrower: an instance admin, or the owner of a private domain, and a domain the agent cannot see is answered as one nobody registered. With `auth.mcp` off there is nobody to hold a role, so the open tier keeps exactly the powers it had - `add_domain` and `configure` are served as before - with one exception: `remove_domain` is new and refuses the open tier outright, because ending a domain is not something nobody in particular does. A local stdio session is the machine owner throughout and is never gated here at all.

**One removal is destructive, and every surface refuses it until it is confirmed.** A file or team domain's files are never deleted by a removal, so unregistering one loses nothing: the folder stays and registering it again re-adopts it (a team domain is reconnected with its repository rather than with its folder, which is what keeps its origin). A virtual domain's engrams live in the database and go with it, so the engine refuses that removal unless the request says the loss was intended - `purge: true` on the `remove_domain` tool, `?purge=true` on `DELETE /api/v1/domains/{domain}` (answered `409` without it), and `--purge` on `crystalline domain remove`. Fluid sends it after its own confirmation. The rule is the engine's rather than any one client's, because this route serves a private domain's owner now and not only an admin.

```mermaid
flowchart LR
    A[Agent] -->|Authorization: Bearer cmt_...| G{MCP gate}
    G -->|no token, or an invalid one| R[401 WWW-Authenticate: Bearer]
    G -->|valid token| S[Session bound to the account]
    S --> D[Daemon: reads and writes as that account]
```

## MCP clients over OAuth

`auth.oauth` turns Crystalline into an OAuth 2.1 authorization server for its own MCP endpoint, so a hosted client - Claude.ai's remote connector first of all - reaches this instance without a person pasting a token into it. The client discovers that the endpoint is protected, registers itself, sends the person to sign in and consent, and from then on holds a credential the MCP gate checks exactly like a personal MCP token from [Authenticated agents](#authenticated-agents). The identity provider is whatever this instance already signs people in with - local accounts, [Enterprise SSO](#enterprise-sso), or [Proxy forward auth](#proxy-forward-auth) - since the consent page is an ordinary signed-in Fluid page: whoever can sign in there can consent.

`auth.oauth` is tri-state, unset by default. Unset, it follows `auth.mcp`: a shared instance where every agent has to authenticate and the UI is served gets OAuth for free, no second switch to flip - see [A shared instance in three steps](#a-shared-instance-in-three-steps) below. Set it `false` (or `CRYSTALLINE_AUTH_OAUTH=false`) to keep issuing personal MCP tokens only, whatever `auth.mcp` says. Set it `true` to insist: it then needs `auth.mcp` on, since the tokens it issues are checked at that gate; `service.api`, since its endpoints live under `/api/v1`; and `service.ui`, since consent happens on a Fluid route - with the UI off, `/authorize` falls to the MCP transport and nobody can ever consent - and the daemon refuses to start against any of the three missing, naming which one. A followed value never trips those refusals, since it only ever turns on where all three already hold. Wherever `auth.oauth` ends up off, the two well-known documents below answer `404`.

**Discovery.** Two documents, mounted at the origin root beside `/health` rather than under `/api/v1`, since the specification places them there: `GET /.well-known/oauth-protected-resource` names this endpoint as a protected resource (its `resource` and the one entry in `authorization_servers`, both the instance's own origin), and `GET /.well-known/oauth-authorization-server` names the three OAuth endpoints below plus what this server supports (authorization code and refresh token grants, PKCE `S256` only, public clients only). A client that shows up with no credential is refused the way [Authenticated agents](#authenticated-agents) already describes, with one addition: the `401`'s `WWW-Authenticate` header carries `resource_metadata` pointing at the protected-resource document, and the teaching text in the body says a client that speaks OAuth can connect without a token at all.

**The origin.** Both documents, the challenge above, and every access token's audience all name the same address: this instance's origin (scheme, host, optional port, no path). Derived per request from the incoming `Host` by default, the same rule the [Enterprise SSO](#enterprise-sso) callback address uses - and overridden the same way, by `auth.oidc.redirect_uri` when that key is set, since it is the one place an operator behind a Host-rewriting proxy has already named the public address (see that key's row in [the environment table](#configure-through-environment-variables)). A deployment that rewrites `Host` and has not set that key mints tokens for an address nobody outside the machine can reach. Separately, the public host itself has to be on `service.allowed_hosts` (the transport validates the `Host` header; see [Configure through environment variables](#configure-through-environment-variables)): a `Host` this instance was not told to trust gets the MCP transport's ordinary `403 Forbidden` there, whatever the well-known documents themselves say - they publish an origin unconditionally and are not behind that guard, so a client can read a resource identifier it then cannot reach. A proxy that forwards the public hostname needs it listed even when it also sets `auth.oidc.redirect_uri`.

**The endpoints**, all under `/api/v1` and public by path - no session or MCP token is needed to reach any of them, since none of them exist yet for a client that has not registered:

| Route | What it is for |
| --- | --- |
| `POST /api/v1/oauth/register` | Dynamic client registration (RFC 7591): a client posts its redirect URIs and gets back a `client_id` (`coc_` plus hex), no secret - only public clients exist here |
| `GET /api/v1/oauth/authorize` | Starts an authorization: validates the client, redirect URI and PKCE challenge, then redirects the browser to the Fluid consent screen |
| `GET /api/v1/oauth/authorizations/{id}` and `POST /api/v1/oauth/authorizations/{id}` | The consent screen's own read and decision, guarded like any other Fluid page - the account making the decision has to be signed in |
| `POST /api/v1/oauth/token` | Turns an authorization code into a token pair, or a refresh token into a fresh one |

**Registration is open, and bounded rather than gated.** Anything may call `POST /api/v1/oauth/register`, the way any browser may load a login page: at most 30 registrations per 10 minutes per process, a 64 KiB request body, and at most 1000 stored registrations at once. A registration nobody ever brings to consent is pruned an hour after it was made, so an anonymous caller filling the table costs an operator nothing to wait out; one that has completed at least one consent is pruned only after 30 days with no further use. `client_name` is optional and shown on the consent screen (default "an MCP client" when a client sends none); `client_uri` and `redirect_uris` are the rest of what a client controls about how it is described.

**What the consent screen shows.** The signed-in account's name, the client's name and (when it set one) its own URL, the host its redirect address points at - with a warning when that host is this machine itself rather than a service on the web, which is what a loopback redirect during local development looks like - and how long the request has left before it expires. There is exactly one decision to make: Allow or Deny, for the whole account, until revoked; there are no scopes narrower than that to choose among. The page carries `Content-Security-Policy: frame-ancestors 'none'` and `X-Frame-Options: DENY` like every other Fluid route, worth naming here because this is the one page in the product where a single click hands a client everything an account can do, exactly the target clickjacking goes after.

**Tokens, lifetimes and rotation.** A successful exchange returns an access token (`coa_` plus hex, one hour) and a refresh token (`cor_` plus hex, thirty days from issue). Every refresh rotates both: the predecessor stops working immediately, and the new refresh token's thirty days start again from that moment - so a client that refreshes regularly never has to send anyone through consent again. Presenting a refresh token that has already been rotated past is treated as a stolen or replayed credential, not an ordinary invalid one: it revokes the whole grant, so a client that used a stale refresh token has to be re-authorized from `/oauth/authorize` like the first time. PKCE with `S256` is mandatory on every authorization; the code it produces is single use and lives 60 seconds, long enough to reach the token endpoint and no longer.

**Connect a hosted client - Claude.ai's connector, as the worked example.** In Claude.ai's connector settings, add a custom connector and paste this instance's origin with no trailing slash (the metadata's `resource` has to equal the address the person typed exactly, and a client that adds a path or a slash where the person didn't will not match it). The connector registers itself against `/oauth/register` on the spot; there is nothing to create by hand first. It then opens this instance's sign-in - whichever of local accounts, single sign-on or a forward-auth proxy this instance is configured with - and lands on the consent screen described above. Allow, and Claude.ai holds a token pair and connects over MCP as that account from then on, refreshing on its own. The connection shows up afterward on the account's own profile, under the Connected clients card next to Agent access - client name, redirect host, when it was granted, when it was last used, and a Revoke button for ending it early, the same place a personal MCP token is issued and revoked.

**Forward-auth proxies and OAuth are unrelated, and both may run at once.** The consent screen is a browser page like any other, so a forward-auth proxy in front of this instance ([Proxy forward auth](#proxy-forward-auth)) may sign the person in for it exactly as it signs them into everything else; the MCP gate itself never reads `Remote-*` headers, whichever credential opens the session. What the proxy must not do is authenticate the machine-to-machine legs: `/api/v1/oauth/token`, `/api/v1/oauth/register` and both well-known documents are public by path and have to reach this instance unauthenticated. See [Proxy forward auth](#proxy-forward-auth) for the one footgun in getting that wrong.

**Pending authorizations and codes live in memory, not the database.** A daemon restart between the redirect to the consent screen and the code exchange loses the request; the client simply starts over from `/oauth/authorize` (or, in practice, from the person clicking connect again), since it was already going to open a browser. Issued tokens are different: they live in the accounts database beside personal MCP tokens and survive a restart untouched, so an ordinary restart never disconnects an already-authorized client.

```mermaid
flowchart LR
    C[Client] -->|1. connect, no token| G{MCP gate}
    G -->|2. 401 + resource_metadata| C
    C -->|3. GET well-known documents| M[Metadata]
    C -->|4. POST /oauth/register| M
    C -->|5. GET /oauth/authorize| F[Fluid: sign in, then consent]
    F -->|6. code| C
    C -->|7. POST /oauth/token| M
    M -->|8. coa_... access token| C
    C -->|9. Authorization: Bearer coa_...| G
```

### A shared instance in three steps

This is what following `auth.mcp` buys a [Team server with Fluid](#team-server-with-fluid): nobody sets `auth.oauth` at all, and hosted clients connect anyway.

1. **Create the first admin.** [Team server with Fluid](#team-server-with-fluid) covers both ways to take that bootstrap step - `users add` from the host, or the create-first-admin form in Fluid.
2. **Set `auth.mcp` true**, so every agent over HTTP MCP presents a credential instead of connecting on the open tier: see [Authenticated agents](#authenticated-agents).
3. **Nothing.** `auth.oauth` is unset, so it follows: the UI is serving (the default) and every agent now has to authenticate, so OAuth is on. A hosted client such as Claude.ai connects through the sign-in and consent flow above with no token pasted anywhere; a harness that cannot speak OAuth keeps using a personal MCP token from step 2's world, issued in Fluid or with `crystalline users mcp-token`. Either way, whichever human sign-in this instance uses - local accounts, `auth.oidc` for an external provider such as Authelia or Keycloak, or `auth.proxy_headers` - is what the consent page rides on, since it is an ordinary signed-in Fluid page: nothing about OAuth itself asks which one an instance chose.

To keep tokens only on a shared instance, set `auth.oauth: false` and stop there - `auth.mcp` still governs agents, only the OAuth surface is off.

## Enterprise SSO

Setting `auth.oidc.issuer`, `auth.oidc.client_id` and `auth.oidc.client_secret` (or the three matching `CRYSTALLINE_AUTH_OIDC_*` variables) turns on a sign-in button beside the local one. It is opt-in and additive: local accounts keep working, and an instance with none of the three set behaves exactly as it did before. All seven keys are read once when the HTTP surface starts, like `service.read_only`, so a change takes effect at the next start; a block missing any of the three required keys leaves single sign-on off and says which key is missing in a startup warning rather than refusing to come up.

Three routes come with it, all public because a browser reaching them has no session yet, and all under the existing `/api/v1` mount, so a reverse proxy that already forwards `/api/` needs no new rule:

| Route | What it is for |
| --- | --- |
| `GET /api/v1/auth/providers` | Whether to draw the button, and its label. Carries no issuer, client id or secret |
| `GET /api/v1/auth/oidc/login` | Starts the sign-in: redirects to the provider with PKCE (S256), a state and a nonce |
| `GET /api/v1/auth/oidc/callback` | Where the provider sends the browser back |

**Register the callback with the provider as the redirect uri**, spelled with the scheme and host your users reach the instance at: `https://<your-host>/api/v1/auth/oidc/callback`. Crystalline derives that address from each request's own `Host` header, and the scheme by one rule: a loopback `Host` (`localhost`, `127.0.0.1`, `[::1]`) with no forwarded scheme is `http`, and everything else is `https` - which is the same rule that decides whether a session cookie gets `Secure`. So a reverse proxy that passes the public hostname through as `Host` needs nothing more; a proxy that rewrites `Host` to `127.0.0.1:7411` must also set `X-Forwarded-Proto: https` (or RFC 7239 `Forwarded: proto=https`), or the redirect uri is built with `http://` and the provider refuses it. The rule has one consequence worth stating plainly: an instance served over plain HTTP on a non-loopback address gets an `https://` redirect uri it does not serve, so single sign-on is not supported on plain-HTTP deployments off loopback. An instance reachable under more than one hostname needs every one of them registered.

**Where that derivation cannot work, name the address instead.** `auth.oidc.redirect_uri` (or `CRYSTALLINE_AUTH_OIDC_REDIRECT_URI`) is used verbatim, and the request's own `Host` is not consulted at all: it has to be an absolute `https` url ending in `/api/v1/auth/oidc/callback` - `http` is allowed only on loopback, for a development server - and it is refused at the moment it is set, with teaching text naming the key, rather than at the provider. That is the key for a proxy that rewrites `Host` to an internal address and cannot be made to forward the public one, and for a deployment whose public entrance simply is not what the process sees. One address, so an instance reachable under several hostnames still derives per request instead.

**With `auth.oauth` on, that key also names this instance to OAuth clients.** The origin of `auth.oidc.redirect_uri` - its scheme, host and port, with the callback path dropped - is the OAuth resource identifier: the `resource` and `issuer` both well-known documents publish, the address in the `WWW-Authenticate` challenge a refused agent reads, and the audience every access token is minted for and checked against. Unset, the identifier is derived from each request's `Host` by the same rule the callback address uses. So a deployment behind a proxy that rewrites `Host` wants this key set even with single sign-on switched off: without it, tokens are minted for the upstream's `127.0.0.1:7411` and the published `resource` is an address nobody outside the machine can reach. A value that is not an absolute `http` or `https` url falls back to per-request derivation with a startup warning, rather than publishing something unusable.

**A first sign-in creates the account it signs in.** The durable key is the pair of the provider's issuer and the token's `sub` claim, and nothing else: a subject nobody has seen before gets a fresh account at `auth.oidc.default_role` (`viewer` unless you set it), with a login name derived from `preferred_username`, or from the local part of the address when the provider sends no username, folded and uniquified with `-2`, `-3` and so on when the name is taken. The account carries no password, so it signs in through the provider only, and it counts against `auth.max_users` like any other. A subject that has signed in before returns to the same account whatever the provider now calls that person: only the display name and the address follow a rename, never the login name, the role or the disabled flag, and a disabled account is refused the same way it is refused everywhere else.

**An address never joins an existing account.** An account created locally, and an account created by an earlier sign-on, are reached by their identity link or not at all: a provider that will hand anybody an `email` claim cannot hand anybody somebody else's account, and a person who has both a local account and a provider identity gets two accounts until the two are deliberately linked.

**Linking an identity to an existing account is an explicit act, by the person or by an administrator.** There are exactly two ways, and neither of them is an address matching:

- **From the profile.** A signed-in account presses "Link" on the SSO identity card. That is a `POST /api/v1/auth/oidc/login` carrying the session's CSRF token, answering with the provider's authorization endpoint in `location` for the browser to navigate to; the sign-on then runs as usual and the callback ties the identity to the account that started it. A POST rather than a link on purpose: the session cookie is `SameSite=Lax` and would ride a cross-site top-level navigation, so a GET that started a link could be started by any other page on the internet. `GET /api/v1/auth/oidc/login` stays the ordinary sign-in, and the same GET carrying `link=true` is refused with a 400 that says where linking lives. The account that started the link has to be the account that finishes it, resolved the same way on both ends (the session cookie, or the trusted header on an instance that uses one): a callback arriving with no live account is refused with 401 (sign in again and start over), one arriving on another account's session with 409, and an identity another account already holds with a 409 that never says which account that is - an operator can move it, and a sign-on must not become a probe for who holds an identity here. The same card lists what is linked (`GET /api/v1/me/identity-links`) and gives one up (`DELETE /api/v1/me/identity-links/{issuer}`, the issuer percent-encoded as one path segment).
- **From the command line.** `crystalline users link <name> --issuer <issuer> --subject <sub>` ties a pair to an account, and `crystalline users unlink <name> --issuer <issuer>` takes it away. This is the repair for a provider that re-issues its subjects - an Entra tenant re-registration, where every `sub` changes and every account is suddenly unreachable: unlink the stale identity, then link the new subject to the same account. Both commands write the accounts database directly, so they work whether or not a daemon is running.

**An account is never left with no way in.** An account a first sign-on provisioned has no password, so its identity link is its only credential, and unlinking the last one is refused - on the profile card and on the command line alike, in one sentence that names `crystalline users passwd`. Give it a password first, or, when the repair genuinely needs the account unreachable for a moment, pass `--force` to `crystalline users unlink`: it says what it left behind, and the account stays unreachable until it is linked again or given a password.

The client secret is stored but never rendered: `crystalline config show`, the `configure` tool and `crystalline doctor` all print `(set)`, and a failure from the provider reaches the browser as a sentence written by Crystalline, with the provider's own words going only to the daemon's `debug` log. Set the secret through `CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET` where a deployment keeps credentials out of `config.yaml` entirely. An `http://` issuer anywhere but loopback is accepted with a startup warning, because the secret and every ID token then cross the network in clear text.

What an operator sees in the daemon's log at its default level: every refused sign-in is one `WARN` line naming the reason category (a state that did not match, an expired sign-in, a token whose signature, issuer, audience, expiry or nonce did not validate, a discovery or key-set fetch that failed, a userinfo answer for another subject) and the issuer, and every validated sign-in is one `INFO` line naming the issuer and the subject (the pseudonymous key, which is what makes a sign-in traceable). No refusal line carries a state, a code, a nonce, a token or the secret, and no line at any level carries a presentation claim - a name, a username or an address. The `debug` lines that render a validation failure carry the validator's own text, which for a mismatched issuer or an expired token names the value in question (`iss`, `exp`) - a refusal an operator cannot locate is not much of a refusal - and `debug` is off at the default level. A provider that keeps `preferred_username`, `name` and `email` out of the ID token (Authelia 4.38 and later without a claims policy) has the missing ones fetched from its userinfo endpoint; an unreachable userinfo costs the sign-in those fields, not the sign-in. Two more limits protect the public login route: at most 10,000 sign-ins may be in flight at once, and past that a new one is answered `503` rather than evicting somebody mid-consent, and a provider whose discovery fails is not asked again for five seconds.

```mermaid
flowchart LR
    B[Browser] -->|1. GET /auth/oidc/login| D[Daemon]
    D -->|2. 302 with PKCE, state, nonce| B
    B -->|3. authenticate| P[Identity provider]
    P -->|4. 302 back with a code| B
    B -->|5. GET /auth/oidc/callback| D
    D -->|6. code plus client secret| P
    P -->|7. ID token| D
    D -->|8. session cookie, 302 to /| B
```

**Notes for Microsoft Entra ID**, gathered in one place since a tenant's setup touches all four of the following:

- **Tenant-specific issuer.** `auth.oidc.issuer` has to be the tenant-specific `https://login.microsoftonline.com/<your-tenant-id>/v2.0`, never the `common`, `organizations` or `consumers` endpoint a portal page shows by default (its literal `{tenantid}` template breaks issuer checks and is refused with that correction, and so are the three tenant-independent aliases). See the `CRYSTALLINE_AUTH_OIDC_ISSUER` row above.
- **A certificate over a secret.** Entra can authenticate a confidential client with a certificate credential instead of a client secret, and an organization's app-registration policy may require it. Crystalline speaks `auth.oidc.client_secret` only in 0.18.0 - register a client-secret credential for the app, or hold off on certificate-only tenants until that support lands.
- **Pairwise subjects move.** An Entra `sub` is tenant- and application-specific by default (pairwise), and it can change on a tenant reconfiguration - an app re-registration, a directory merge - which strands every account that first sign-on ever provisioned. The repair is the linking commands above: `crystalline users unlink <name> --issuer <issuer>` takes the stale identity off, then `crystalline users link <name> --issuer <issuer> --subject <new-sub>` puts the new one back on the same account, with `--force` on the unlink half when the account has no other way in for the moment between them.
- **App Roles over the `groups` claim.** Crystalline reads no group or role claim in 0.18.0 - every account provisioned through single sign-on lands at `auth.oidc.default_role`, whatever the person's Entra groups or App Roles say. If claim-based role assignment is ever added, App Roles is the better source to build it on than the `groups` claim: it rides the ID token directly, where `groups` needs a Microsoft Graph lookup past 200 memberships and is easy to get wrong at tenant scale.

## Proxy forward auth

A proxy that has already authenticated the person - a forward-auth deployment, the shape [Authelia](https://www.authelia.com/) or [oauth2-proxy](https://oauth2-proxy.github.io/oauth2-proxy/) sit in front of an application as - can name them to Crystalline in the standard `Remote-*` quartet instead of anybody signing in twice. `auth.proxy_headers` (or `CRYSTALLINE_AUTH_PROXY_HEADERS=true`) turns that on, and it is off in every install that has not asked for it: with the mode off, a request carrying `Remote-User` is resolved exactly as one carrying nothing at all, so a header a client made up buys nothing.

**This is trust-the-proxy, and the trust is total.** With the mode on, whoever can set `Remote-User` on a request that reaches this instance is whoever they say they are. That is safe under exactly two conditions, and both of them are the operator's to hold:

- Crystalline is unreachable except through that proxy. A port that is also reachable directly is an open door, not a protected one.
- The proxy strips client-supplied copies of all four headers and sets them itself. A proxy that forwards what a client sent (nginx does, unless told otherwise) hands every browser the ability to name any account.

There is nothing Crystalline can check about either one, which is why the mode is opt-in and why this paragraph is where it is.

| Header | What it is used for |
| --- | --- |
| `Remote-User` | The person. The identity key, and the only header that matters for who the request is |
| `Remote-Name` | The display name, refreshed when it changes |
| `Remote-Email` | The address, refreshed when it changes |
| `Remote-Groups` | Parsed and discarded. Claim mapping is not built in 0.18.0: no group decides a role, a membership or a visibility. Read as a list header, so a proxy may send one comma-separated value or one header line per group; the other three are single-valued and a second copy of one of them is refused |

**A forwarded identity gets an account of its own.** The durable key is the pair `("proxy", <the forwarded user, folded>)`, stored in the same identity-link table single sign-on uses, and a first arrival provisions a fresh account at `auth.oidc.default_role` (`viewer` unless you set it, the same key both external-authenticator modes read) counting against `auth.max_users` like any other. It is never an existing local account of the same name: a `Remote-User: ada` beside a local admin called `ada` provisions `ada-2` at viewer and leaves the admin exactly as it was. That is the same no-silent-linking rule single sign-on follows, applied to a mode that asserts nothing but a name - and it means a person who has both a proxy identity and a provider identity has two accounts until somebody links them, with `crystalline users link <name> --issuer proxy --subject <the forwarded user>`.

**Create the admin account before turning this mode on.** The first proxied request provisions an account and so ends the first-run wizard, and if that account is a viewer the instance has no admin and the way back is `crystalline users add --role admin` on the host.

The rest follows the modes already documented above: a disabled account is refused, a request that carries no `Remote-User` falls through to the ordinary session cookie (turning the mode on escalates nobody), the first `/api/v1/auth/me` mints the session whose CSRF token this identity's writes echo, and a `Remote-User` that cannot be a login name is refused `403` naming why rather than provisioning something unusable.

**One header mode at a time.** `auth.trusted_header` names a single custom header and `auth.proxy_headers` trusts the standard set; an instance configured with both leaves the HTTP endpoint refusing to start (a warning naming both keys and saying to pick one), while the daemon itself keeps running - MCP over stdio, and every other background task, is unaffected. Note what switching between them costs: accounts the trusted-header mode created carry no identity link, so an instance that turns that mode off and this one on provisions a second account for every person, and the old accounts (with their roles and memberships) stay behind. Link them rather than re-granting: `crystalline users link <old-name> --issuer proxy --subject <the forwarded user>` makes the account they already have the one the proxy signs them into.

**Agents are governed separately.** `auth.mcp` is what decides whether an HTTP MCP connection has to authenticate, and it is answered with a personal MCP token and nothing else. No `Remote-*` header is a way past that gate, whatever this mode is set to: a proxy vouches for a browser, and an agent presents its own credential.

**This mode and `auth.oauth` coexist by design, and the two are unrelated except at one seam.** With OAuth on ([MCP clients over OAuth](#mcp-clients-over-oauth)), the consent screen a client's browser lands on is signed in the same way every other Fluid page is here, forward-auth included. But `/api/v1/oauth/token`, `/api/v1/oauth/register` and both well-known documents are machine-to-machine legs, public by path and never read through `Remote-*` headers - a proxy in front of an OAuth-enabled instance must pass those paths through unauthenticated. Getting that wrong has a specific failure shape worth knowing before it is met in production: the request guard checks CSRF before it checks whether a path is public, so a proxy that injects `Remote-User` on every request hands the token endpoint a trusted-header identity with no CSRF token yet, which the guard answers `403` - silently breaking every token exchange and refresh a connected client attempts. Exempt `/api/v1/oauth/*` from whatever forwards the identity header at the proxy, rather than routing around it here.

```mermaid
flowchart LR
    B[Browser] -->|1. request| P[Forward-auth proxy]
    P -->|2. authenticate, strip client headers| P
    P -->|3. request plus Remote-User, -Name, -Email, -Groups| D[Daemon]
    D -->|4. look up proxy plus user in the identity links| A[(Accounts)]
    A -->|5. the linked account, or a fresh one| D
    D -->|6. answer, and a session on the first probe| B
```

## The stdio and HTTP surfaces can differ

One daemon can serve a local agent over stdio and a remote one over HTTP at the same time, and with `skills.serve` at its `auto` default those two clients are deliberately not served the same skill surface. An operator who notices that is looking at a decision rather than a bug, so here is the whole of it.

A stdio session runs in a `crystalline mcp` process the harness itself started, so that process knows which harness it belongs to: `crystalline install` registers it as `crystalline mcp --harness <name>`, and at startup it asks the local install receipt whether that harness has session hooks wired. If it has, the harness already carries the five skills as files and delivers the routing block from its own hook, so the session is served neither the skill surface (the `skills` tool, the five `skill://` resources, the two prompts) nor a second copy of the block. Nothing in that decision comes from the connecting client, and it is fixed before the session starts.

An HTTP client gets no such treatment and never will: one daemon serves every HTTP connection, and a remote client never ran `crystalline install` on this machine, so nothing here says what it already has. A remote client is exactly who the served surface exists for.

There is a second asymmetry, and it moves in the other direction: the routing block a stdio session gets on arrival names every registered domain, while the one an HTTP session gets on the legacy `initialize` handshake names none. That is not a setting - it is the handshake's shape. The instructions are built once per connection with no request in hand, so over HTTP there is no caller to resolve and no way to leave a private domain out of the block; the handshake therefore carries every behavior rule, a count of how many domains are registered and a pointer at `list_domains` with `include_routing: true`, which does resolve its caller and does filter. Every channel that carries a request scopes properly: `server/discover` (the 2026-07-28 era's own onboarding path), the `onboarding` prompt and `list_domains` itself each hand an agent the domains its account may see. A stdio session is the machine owner and keeps the whole block. An operator who wants domain lines in an HTTP client's instructions has one answer, and it is the one the block itself gives: have the client call `list_domains` at session start (the `connector` prompt and the shipped skills both teach exactly that).

Two things make the skills difference inspectable and overridable rather than mysterious:

- **See which answer a stdio session will get:** `claude mcp get crystalline` (and the Codex and Copilot equivalents) prints the registered command. If it reads `crystalline mcp --harness <name>` the session can be suppressed; if it reads plain `crystalline mcp` - which is what every registration written before that flag existed reads - it is served the full surface. Rerunning `crystalline install claude-code` repairs a flagless registration in place; it leaves a hand-edited one alone (a command that is not the one it writes, an entry in another scope, an environment block of your own) and prints the command you can run yourself. **It repairs Claude Code only.** Codex and Copilot registrations are read back through their own CLIs, whose `mcp get` output format has not been verified, and an install that cannot read what it is repairing must not touch it, so those are reported and left alone; replace one by hand with `codex mcp remove crystalline && codex mcp add crystalline -- crystalline mcp --harness codex` (the same shape for `copilot`), or set `skills.serve` explicitly and sidestep the whole question.
- **Turn the difference off:** set `skills.serve` explicitly to `true` or `false`. An explicit value always wins over the resolved answer, on both transports, so every client on the machine is served identically. Only the `auto` default produces the asymmetry.

## Read-only deployments

Pass `--read-only` to `serve` (or to `mcp`), set `service.read_only: true` in the config, or set `CRYSTALLINE_SERVICE_READ_ONLY=true` (the container-native spelling, see [Configure through environment variables](#configure-through-environment-variables)) to serve the content API read-only. The seven write-gated tools (`write_engram`, `edit_engram`, `split_engram`, `move_engram`, `delete_engram`, plus `add_domain` and `remove_domain`, which register and unregister domains) disappear from the MCP tool list and are refused if a client calls one by name, while `search_engrams`, `read_engram`, `list_domains` and the rest of the read tools stay. Sync, the file watcher and embedding keep running, so the index still follows external edits such as a git pull. A read-only instance refuses a collaborative editing session too, along with every other write to the knowledge: the collab WebSocket upgrade is refused, so a Fluid page opens an engram to read but never to co-edit. The one exception is the MCP token surface (`/api/v1/me/mcp-tokens` in the API, the Agent access card in Fluid), which keeps working: what `read_only` protects is the knowledge, and a token is account state in the accounts database rather than knowledge - a read-only server with `auth.mcp` on is exactly where an agent cannot connect at all until somebody issues it one. `crystalline prompt system` and the routing block the server hands each connecting agent follow the same mode: both drop the write guidance and state that the knowledge is curated externally, and `prompt system --read-only` forces that variant on demand. The block reaches an agent by whichever channel its protocol revision uses - `initialize` for every revision before 2026-07-28, `server/discover` from then on, where the handshake no longer exists - and carries the read-only variant either way. This is the natural pairing for the git-sync setup in `compose.git-sync.yaml`, where knowledge arrives by reviewed git commits and agents only consume it. The mode is fixed for the daemon's lifetime, so an agent attaching to a running daemon gets that daemon's mode. Most operator tooling on the host (`verify`, `import`, `domain init`/`add` and `model download`) is unaffected: the boundary is that the served API is read-only, not the machine. `crystalline domain remove` is the exception and follows the mode like `crystalline config set` does, because it goes through the same engine entry point every other surface removes a domain through, and a read-only instance's registry is frozen along with its knowledge. That entry point also clears the domain's index rows, so the command needs the index open: on a Postgres deployment a removal while the database is unreachable fails rather than editing the config file alone, which is the right answer in a shared database - the rows are every instance's, and a config-only removal here would leave the domain live for everybody else. The error says as much and names the config file to edit by hand if the database is gone for good. The web UI is served in full, because reading it is all GETs: the daemon hands out the shell and the app renders itself read-only, so a read-only instance is browsable as well as searchable, and with `auth.anonymous: true` it is browsable logged out (see [Web UI from the daemon](#web-ui-from-the-daemon)).

Agents connect to the containerized daemon over its HTTP MCP endpoint, `http://localhost:7411` from the host (the image's default command is `serve --http 0.0.0.0:7411`, since a container has to bind every interface to be reachable at all - binding `127.0.0.1` inside a container is only reachable from inside that same container). The stdio `crystalline mcp` transport (see [Get started](../README.md#get-started) in the README) is for local, non-containerized processes; point a harness at the HTTP endpoint instead when Crystalline runs in a container. A browser reaches the web UI at that same address, since the image serves it built in ([Web UI from the daemon](#web-ui-from-the-daemon)). Every HTTP endpoint also answers `GET /health` with a static `{"status":"ok","version":...}` JSON body, so a load balancer or uptime monitor can probe the daemon without an MCP handshake.

The HTTP transport is Streamable HTTP: every exchange is a POST whose response arrives as a minimal SSE stream carrying exactly the JSON-RPC message, with no optional SSE fields (no retry hints, no priming frames), so strict intermediary parsers - AWS Bedrock AgentCore Gateway among them - consume it cleanly. Streamable HTTP is the transport MCP revision 2025-03-26 introduced, and it carries every protocol revision this server serves: `2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25` and `2026-07-28`. What is not served is the older HTTP+SSE transport that 2025-03-26 replaced: a client opening the old-style GET stream is answered with an immediate `400 Bad Request` naming the missing session id rather than a silent hang. A client declaring a version string this server does not serve is refused at the handshake with the protocol's own `-32022 Unsupported protocol version`, which names the revisions it can retry with, rather than being half-served.

**The session model differs by revision, and an operator meets that at the load balancer.** A client on any revision before `2026-07-28` opens with an `initialize` handshake and is handed an `Mcp-Session-Id` it presents on every later request, so those requests have to reach the same instance. From `2026-07-28` there is no handshake and no session at all: each POST carries its own protocol metadata and is answered on its own, which is what SEP-2575 exists for, so **a modern client needs no session affinity** and any instance can answer any of its requests. The client-opened GET stream is gone in that revision too; a modern client that wants notifications opens a `subscriptions/listen` POST stream instead, and receives on it only the categories it asked for. The daemon's `http_sessions` figure in `crystalline status --json` counts the legacy sessions alone, by construction: a modern client creates none. Results a modern client gets also carry the revision's caching hints, and they authorize nothing: the scope is `public`, but `ttlMs` is `0` on every one, so a caching proxy or CDN in front of the port may share what it holds but may not treat any response as fresh, which means it may not reuse one, and a legacy client is sent no hints at all because the fields do not exist below that revision.

The HTTP transport validates the request `Host` header to block DNS-rebinding attacks, where a malicious web page tries to drive a reachable MCP server from inside the victim's browser. It answers only requests whose `Host` is on its allow-list, which is loopback by default (`localhost`, `127.0.0.1`, `::1`). That default already covers the `http://localhost:7411` access above with no extra configuration, and the bind address is independent of it: binding `0.0.0.0` changes nothing about which `Host` values are accepted. Reaching the daemon by any other name needs that name added - a compose service-name (`http://crystalline:7411`), a LAN hostname or IP, or a public hostname forwarded by a reverse proxy. Add it with `CRYSTALLINE_SERVICE_ALLOWED_HOSTS` (comma-separated) or the repeatable `serve --allowed-host <host>` flag; loopback stays allowed either way. A single `*` accepts any `Host` and turns the guard off, which is only safe behind a trusted reverse proxy or firewall that validates `Host` itself. A blocked request gets `403 Forbidden`; `GET /health` is never guarded, so probes keep working regardless. A reverse proxy that rewrites the upstream `Host` to `localhost` needs no allow-list entry; one that forwards the original public `Host` (the common default) needs that hostname listed.
