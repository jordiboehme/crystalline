# FAQ

Short answers to the questions that come up first.

## Why not just a folder of markdown files?

It is one: that is the point. Your knowledge stays plain markdown you can read, diff and back up with anything. Crystalline adds what a folder cannot: domain routing, hybrid text-plus-semantic search, a knowledge graph and temporal filtering, so the ten-thousandth engram is exactly as findable as the tenth.

Crystalline is the evolution of approaches that many teams have walked through in the same order. Giving an agent a single markdown file of instructions works, until it grows past what fits in context. Splitting it into a folder of markdown files works, until nobody can tell which file to read for a given task. Adding index files that point at folders and other files works, until maintaining the pointers becomes its own job and every lookup still means walking a tree by hand. Each step scales further than the last, and each one quietly breaks somewhere in the hundreds of files.

Once knowledge grows into the thousands or tens of thousands of units, reading and pointer-walking stop being viable at all. What is needed at that scale is what any large knowledge system needs: real indexes. Crystalline keeps the plain markdown files (they remain the source of truth, readable and diffable) and adds domain routing, full-text and semantic search, a knowledge graph and temporal filtering on top.

## Why not a vector database or a RAG framework?

Retrieval is the easy half. A vector index finds similar text, but it does not know which domain owns a task, that a fact was superseded in March, who verified a claim or when something new is worth capturing. Crystalline treats embeddings as one ranking signal inside a knowledge system: routing, temporal semantics, provenance and a capture workflow on top of files you own, with no pipeline to operate.

## Where does the name come from?

From psychology. Crystallized intelligence is the knowledge and skill a person accumulates through education and experience; its counterpart, fluid intelligence, is the on-the-spot reasoning applied to problems never seen before. A model ships with fluid intelligence in abundance and none of your crystallized kind: every session starts as a brilliant stranger. Crystalline is the crystallized half: the store of what an agent has learned, so experience compounds instead of evaporating. (An engram, fittingly, is neuroscience's word for the physical trace a memory leaves.)

## When does the daemon start?

Two ways. Explicitly: `crystalline serve` runs it in the foreground, `crystalline serve --daemon` in the background. Implicitly: the first agent that connects through `crystalline mcp` attaches to a running daemon or starts one on the spot. Either way an advisory lock guarantees a single instance. Every later agent, terminal or CLI command attaches to that one.

## When does the daemon stop?

Only when told to. It does not exit when the last agent disconnects or on idle: watching, embedding and origin polling keep running so the index stays warm for the next session. It shuts down cleanly on `crystalline ctl shutdown`, on Ctrl-C in a foreground `serve` and on SIGTERM (which is how the container image stops). On the way out it releases its host locks and removes its socket and lock files.

## How do I stop it manually?

`crystalline ctl shutdown` from any terminal asks the running daemon to stop cleanly over the local socket. If a crash ever leaves a stale lock or socket file behind, `crystalline doctor --fix` cleans them up. A daemon that is still alive but has stopped answering is replaced automatically by the next client that connects, and `crystalline doctor --fix` forces the same replacement on the spot.

## Is the HTTP endpoint authenticated?

Off by default. `auth.mcp` makes every HTTP MCP connection present a personal token, and a hosted client can sign in over OAuth instead (see [Authenticated agents](deployment.md#authenticated-agents) and [MCP clients over OAuth](deployment.md#mcp-clients-over-oauth)). With it off, the endpoint is on by default at `127.0.0.1:7411`, so on a shared machine any local process can reach MCP there; `crystalline config set service.http false` (or `CRYSTALLINE_SERVICE_HTTP=false`) turns the endpoint off. The web UI and the JSON API on that same port are a separate surface with accounts of their own: the browser shell is served to anyone who connects, while every request for knowledge needs a session, and the first visit to an instance with no accounts is what creates the first one, see [Web UI from the daemon](deployment.md#web-ui-from-the-daemon). That is the trade on the `127.0.0.1` default. The container image binds `0.0.0.0` (see [Run in a container](deployment.md#run-in-a-container)) so agents on the host can reach it, so treat the network boundary around the container (a private network, a reverse proxy, firewall rules) as the access control until you turn `auth.mcp` on. It does validate the request `Host` header to block DNS rebinding: loopback is accepted by default, and any other hostname (a reverse proxy, a LAN name, a compose service-name) must be added with `crystalline config set service.allowed_hosts <host>` (or `CRYSTALLINE_SERVICE_ALLOWED_HOSTS`) so every daemon on the machine accepts it, however it was started. `serve --allowed-host` is the per-invocation override, for that one process only (see [Configure through environment variables](deployment.md#configure-through-environment-variables)).

## Where does my knowledge actually live?

In your domain folders, as plain markdown you can read, edit and back up with anything. By default those folders sit under `~/Documents/Crystalline`, one per domain, which is where a domain lands when nobody names a path. A folder you registered yourself lives where you put it, and the `domains_root` setting moves the default. Everything Crystalline derives from it is disposable: the search index lives in the state directory and `crystalline reindex --full` rebuilds it from the files at any time. The config file, the index and the model cache live in the platform config, state and cache directories (`~/.config/crystalline`, `~/.local/state/crystalline` and `~/.cache/crystalline` on Linux and macOS).

## Do I need git to share knowledge with a team?

No. Team domains talk to GitHub directly over its API: no git, no gh, no local clones. Members connect once with a browser code and Crystalline handles the rest.
