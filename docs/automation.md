# Run the loops on their own

Two loops keep a domain in shape. The reflect loop works a domain's [Reflection](reflection.md): feedback people left on a page, threads waiting for an answer and pages whose engrams changed. The evolve loop works the maintenance findings of [`crystalline evolve`](evolve.md). Both can run without a person starting an agent.

Crystalline never starts a program. A runner outside it does the work: `crystalline reflect run` started by you or by a scheduler, a container job or an n8n workflow. Crystalline answers four questions for that runner: is something due, may I run now (the lease), what did the run change (the share) and how did it end (the run record). It can also send a signed webhook when work becomes due, so a runner starts without polling.

Every runner asks first. When nothing is due, it ends at once and starts no agent.

## The commands

`<loop>` is `reflect` or `evolve`.

| Command | What it does |
|---|---|
| `crystalline <loop> status [--domain D]...` | Shows per domain what is due, as counts, plus a run that is going, the last run and the webhook state. `--json` prints the answer as it came. |
| `crystalline <loop> run [--domain D]` | Works what is due with a headless Claude Code agent: takes the lease, runs the agent with the shipped prompt, shares its changes as a pull request, records the run and releases the lease. Without `--domain` it works every due domain in turn. |
| `crystalline <loop> watch [--domain D]... [--interval S]` | Checks the status every `--interval` seconds (default 60) and runs when something is due. Runs until it gets SIGTERM or Ctrl-C. |
| `crystalline <loop> prompt --domain D [--max-items N]` | Prints the shipped prompt, for a runner that starts its own agent (n8n). |

`run` and `watch` take the same settings:

| Flag | Default | Meaning |
|---|---|---|
| `--max-items N` | 20 | The most items the agent works in one run. |
| `--max-turns N` | 60 | The most agent turns in one run. |
| `--model M` | Claude Code's default | The model the agent uses. |
| `--log-dir P` | `runs/` in the state directory | Where the agent's stream is written, one file per run, readable by you only. The newest 50 per loop and domain are kept. |
| `--runner LABEL` | `cli` | The label the lease and the run record show, for example `k8s` or `n8n`. |
| `--no-share` | off | Keep the run's changes local instead of sharing them. |
| `--dry-run` | off | `run` only: show the command it would start, take no lease and start no agent. |
| `--force` | off | `run` only: run even while the loop backs off after failed runs. |

Exit codes, so a shell `if` reads naturally:

| Command | 0 | 1 | 2 |
|---|---|---|---|
| `status` | something is due | nothing is due | an error |
| `run` | the run is done or stopped at its item cap | nothing is due, or a run already holds the lease | the run failed, or an error |

A scheduler reads every code that is not 0 as a failed job. The Kubernetes and GitHub examples use a small wrapper, [`examples/kubernetes/crystalline-loop.sh`](../examples/kubernetes/crystalline-loop.sh), that turns 1 into 0 and passes SIGTERM on to the run.

### Local or remote

Without anything set, the commands talk to the daemon on this machine, and the agent reaches Crystalline through the stdio entry `crystalline mcp`. With no daemon running they fail with "Start the daemon (crystalline serve)".

With `CRYSTALLINE_REMOTE_URL` and `CRYSTALLINE_REMOTE_TOKEN` set (or `CRYSTALLINE_REMOTE_TOKEN_FILE`), they talk to that server instead, through its ctl route (`POST /api/v1/ctl`) as the token's account. The agent then reaches the server's MCP endpoint, which is the root of that URL, with the token as a bearer header. A runner job needs nothing but the binary, Claude Code and those two variables.

The server checks the `Host` header of every request. When a runner reaches the server by a name people do not use (a cluster service name, a container name), add that name to the server's `CRYSTALLINE_SERVICE_ALLOWED_HOSTS`.

### What `run` starts

`run` needs `claude` on `PATH`; without it, it names the [install docs](https://code.claude.com/docs/en/setup). It writes the agent's MCP config into a new folder only you can read and starts:

```sh
claude --mcp-config <config> --strict-mcp-config --setting-sources "" --tools "" \
  --allowedTools mcp__crystalline --disallowedTools <the forbidden tools> \
  --output-format stream-json --verbose --max-turns 60 -p "<the shipped prompt>"
```

So the agent sees only the Crystalline MCP server, no built-in tools, no settings files and no hooks. The local MCP config starts `crystalline mcp --unattended --domain <domain>`, one agent per domain. Against a server the config carries the bearer and the header `X-Crystalline-Run-Domain: <domain>`, which the server believes only while that account holds the run's lease there. Both mark the session as one nobody is behind and bind it to the domain the run works on: it is never asked to share Reflection text from a domain fewer people may read, and it reads only the domains every reader of that domain may read (see [What an unattended run may read](reflection.md#what-an-unattended-run-may-read)). You set none of this: `run` writes it into the config. Custom automation that starts its own agent marks it the same way. When something is due and `claude` is on `PATH`, `crystalline reflect run --dry-run` prints the command, with placeholders for the config and the prompt. These flags were checked against Claude Code 2.1.312.

The agent gets no Crystalline credentials in its environment: every variable that starts with `CRYSTALLINE_` and holds `TOKEN`, `SECRET` or `PASSWORD` is removed. The remote bearer is in the MCP config only, and the config with its folder is removed when the run ends, also after an error or a stop. Your Anthropic credential (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`) is passed on as it is.

`run` renews the lease every 60 seconds while the agent works, reads the summary block at the end of the agent's last message, shares, records the run and releases the lease. The outcome is `done` when the agent ended with a summary below its cap, `stopped_at_cap` when it reached the cap and `failed` otherwise (no summary, an agent exit code that is not 0, a stop or a lost lease).

## Who may run a loop

A run acts as an account. For a runner job or n8n that is an agent account with a `cmt_` token (see [An account per agent](deployment.md#an-account-per-agent)). With a GitHub App, access to a team domain follows GitHub, so the agent account needs a GitHub link too. An admin runs `crystalline connect github --personal --as <agent-account>` on the server and signs in to GitHub, in the device flow, as a bot user with write on the repository. The device flow must be switched on in the App's settings on GitHub. Write gives editor and lets the account curate the Reflection, which is enough for reflect and evolve.

- The reflect loop needs the right to curate the domain's Reflection.
- The evolve loop needs the right to write the domain.

`status` without `--domain` shows only the domains the account may run the loop on.

Reflect runs on one instance per domain, the domain's Reflection home (see [One home, one backup](reflection.md#one-home-one-backup)). Evolve on a team domain runs on the domain's evolve home: the instance whose `service.public_url` is the MANIFEST's `evolve_home`. The first scheduled evolve run at an instance with a `service.public_url` claims it, and its pull request carries the line (see [One evolve runner per team domain](teams.md#one-evolve-runner-per-team-domain)). Every other clone shows `runs on <home>` and is never due, and `run` refuses there. Until an instance claims the home, every clone may run evolve; the run says so, and `crystalline doctor` warns. When instances share one Postgres database, the home is asked first, then the host lock: evolve runs on the instance that holds the domain's host lock among the instances that are the home. Instances of one database usually serve one address, so they all are. When the home names another address than the lock holder has, nobody in the group runs, and each names the other; give the instances one `service.public_url` or move the home. A local domain has no evolve home: without a shared database every instance with its own copy of it may run evolve.

Give the reflect runner and the evolve runner separate accounts when both work one domain at the same time. Crystalline cannot tell which run made an MCP write, so it notes the write in every lease the account holds on that domain, and each pull request would carry the other run's writes.

## Headless means nobody to ask

The shipped prompts (`crystalline <loop> prompt` prints them) tell the agent that nobody is there to answer:

- Start by loading the skills through the `skills` tool.
- Never wait for a yes and never ask a question in the session. The reflect agent asks people in a Reflection thread with `discuss`. The evolve agent names its questions in the summary.
- Stop at the item cap. What it did not reach stays due for the next run.
- End with a fixed summary block: `Done: <n>`, `Left for a person: <n>`, `Errors: <n>` and a few lines that name each item it left and why.
- Reflect works the queue. Evolve asks `evolve_engrams` for `mechanical` findings only and never touches a `judgment` finding.
- A tool answer that says the domain was renamed in its MANIFEST is left for a person and named in the summary.

The agent may not call these tools, and `run` passes the same list to `--disallowedTools`: `delete_engram`, `move_engram`, `split_engram`, `remove_domain`, `add_domain`, `configure`, `provision`, `share_changes`, `update_domain`, `resolve_conflict`, `withdraw_proposal` and `discard_changes`. Two more rules are parameters, not tools, so they are rules of the prompt only: no `write_reflection` action `delete_asset` and no `write_engram` with `overwrite: true`.

### What a headless run leaves for a person

- Every item that needs a yes: a judgment finding, a delete, a move, a page a person wrote.
- Every item that needs a person's answer: the agent asks in a Reflection thread and leaves the thread open for that person.
- A file only a person can add.
- A pending rename question.

A `done` run records each item that was due when it started and is still due when it ends, at its version at that moment. `status` leaves such an item out of what is due and counts it as "left for a person" instead, until the item changes: a new message in the thread, a new edit of the feedback, a new source change, new evidence for the finding. So an item no run can settle does not start a run every 5 minutes all day. A run that stopped at its cap, failed or expired leaves nothing for a person, so what it did not reach stays due.

## Sharing as a pull request

On a team domain with a GitHub origin, `run` itself (not the agent) shares the run's changes as a pull request, whatever the domain's `sharing` says. So a headless change is always reviewed. The title names the loop, the domain and the date, for example `Evolve run on eng, 2026-10-22`. The description is the run summary. The run record keeps the pull request's address. A second run while the first pull request is still open adds to it.

- Only the runner's own MCP writes are shared: the paths the lease's account wrote through the MCP tools while it held the lease. A write through the JSON API, Fluid, the files or a sync is never part of it, also when it is made under the same account.
- Reflection pages are never part of it. The home backs them up to the `crystalline/reflections` branch as always.
- A file a person had unshared work on when the run started is held back, even when the run changed it too. The answer lists it in `held_back`, and the pull request says how many files were held back, so a person shares them.
- The changes of a run that was stopped are not shared, by this run or by a later one. They stay local for a person.
- A domain with no origin, or an instance with GitHub off, shares nothing (`no_origin`). `--no-share` keeps everything local too.

A local `run` against your own daemon acts as the machine owner, not as an account, so it cannot tell its own writes from yours. It shares the files that are new or changed since the lease was taken instead, still holding back what you had unshared at the start. Do not edit the domain on that machine while such a run works it, or give the runner its own account and run it in remote mode.

## The lease and the run record

Each domain has one lease per loop. A reflect lease and an evolve lease never block each other; two runs of the same loop on one domain never overlap. A runner renews its lease every 60 seconds. A lease that was not renewed for 5 minutes is stale: its runner is taken for crashed, the loop is due again and the next run takes the lease over and records the old run as `expired`.

The lease token is a bearer secret. Whoever holds it can renew, share and record for that lease. Keep it out of logs.

A run record keeps when the run started and ended, the runner label, the account, the outcome (`done`, `stopped_at_cap`, `failed`, `nothing_due` or `expired`), the items done, the items left for a person, the judgment findings waiting (evolve), a summary of at most 2 KiB and the pull request. The last 50 runs per domain and loop are kept in the index, so a full reindex that rebuilds the index loses them.

After a failed run the loop backs off on that domain: status says `backing off after N failed runs` and is not due, and the webhook waits too, until 5 minutes after the last failure, doubled for each further failed run in a row and at most 6 hours. A run that did not fail ends the wait; an `expired` run neither counts nor ends it. `crystalline <loop> run --force` starts a run anyway.

A stop signal (SIGTERM or Ctrl-C) stops the agent, records the run as failed and releases the lease. All of it ends within about 25 seconds, so a Kubernetes pod with the default grace period of 30 seconds records its run. A run that cannot record leaves a lease that goes stale by itself within 5 minutes.

If the local daemon dies during a run, the agent's `crystalline mcp` bridge may start a new daemon. That daemon has the agent's filtered environment, so it has no `CRYSTALLINE_GITHUB_TOKEN` and no admin password until it is restarted.

## On your machine: `watch`

`crystalline reflect watch` is a long-running process for a login item or a user service. It needs the daemon. As a systemd user unit, `~/.config/systemd/user/crystalline-reflect.service`:

```ini
[Unit]
Description=Work the Reflection queue of eng when something is due

[Service]
Environment=PATH=/usr/local/bin:/usr/bin:/bin:%h/.local/bin
ExecStart=/usr/local/bin/crystalline reflect watch --domain eng --max-items 10
Restart=on-failure

[Install]
WantedBy=default.target
```

`claude` must be on the `PATH` of the run; Claude Code's own installer puts it in `~/.local/bin`. Turn the unit on with `systemctl --user enable --now crystalline-reflect`. Or start a single run from cron, which ends at once when nothing is due. Cron's `PATH` is short, so name the folders:

```bash
PATH=/usr/local/bin:/usr/bin:/bin:/home/you/.local/bin
30 6 * * * crystalline evolve run --domain eng >> /home/you/crystalline-evolve.log 2>&1
```

A local run acts as the machine owner (see [Sharing as a pull request](#sharing-as-a-pull-request)), so use it on a machine only you use. On a team server, use remote mode with an agent account, as [A scheduled run](reflection.md#a-scheduled-run) shows for systemd.

## At home: n8n

[`examples/n8n/`](../examples/n8n/) has four workflows to import into n8n:

- `reflect-loop.json` and `evolve-loop.json`: a Schedule trigger at a fixed time (reflect every day at 06:00, evolve every Sunday at 03:00), an optional Webhook trigger with the signature check, the status check, the lease, an AI Agent with the MCP Client Tool, the share and the run record.
- `renew-lease.json`: a small workflow the two loop workflows start, without waiting for it, right after they take the lease. It renews the lease every 60 seconds while the agent works.
- `capture.json`: send text to a webhook, and an agent writes it as an engram with `write_engram`.

Each workflow has a note with its setup steps. In short:

1. Create a Bearer Auth credential with the `cmt_` token of an agent account and pick it on every HTTP Request node and on the MCP Client Tool. The MCP Client Tool's endpoint is the server root URL with a trailing slash, over HTTP Streamable.
2. Create an Anthropic credential and pick a model.
3. Edit the Settings node: the server address, the domain and the item cap.
4. Run `crystalline reflect prompt --domain eng --max-items 10` and paste what it prints into the AI Agent's prompt. Paste it again after a Crystalline upgrade, since the prompt may change.
5. Import `renew-lease.json`, pick the credential on its HTTP Request node and pick that workflow on the node Keep the lease fresh.

The MCP Client Tool leaves out the twelve tools a headless run may not call. The HTTP Request nodes send the ctl verbs the CLI sends: `automation_status`, `automation_lease_take`, `automation_lease_renew`, `automation_share` and `automation_run_record`, each a `POST /api/v1/ctl` with a body such as `{"v": 1, "cmd": "automation_status", "loop": "reflect", "domains": ["eng"]}`. The answer is `{"v": 1, "ok": true, "data": ...}`, or `ok: false` with an `error` such as `already running`.

An instance has one webhook URL (`automation.webhook.url`) for both loops and every domain whose trigger is on. So only one of the two loop workflows takes the webhook, and the other runs on its schedule. Its signature check drops an event for the other loop, and a webhook for another domain only starts a status check of the domain in Settings.

An n8n node cannot renew the lease while the AI Agent node works, so the renewing runs in `renew-lease.json` beside it. It stops when a renew is refused: the run was recorded (which releases the lease) or another run took it over. It also stops after 2 hours. A failed call is tried 3 times, 5 seconds apart. If that fails too, the renewing stops and the lease is stale 5 minutes after the last renew. The next schedule or a webhook may then start a second run that takes the lease over. Two agents then work the same queue, the first run's share and record are refused (`ok: false`), and the writes the first run made are in no pull request. Keep the item cap and the agent's iterations small. For long runs use `crystalline <loop> run` in a container.

## At work: a runner job

[`examples/kubernetes/`](../examples/kubernetes/) has a runner image and three manifests. They run in remote mode against the team server.

- `Dockerfile`: Claude Code 2.1.312 on a Node base image, plus the `crystalline` binary copied from the published image. The published image is distroless, so nothing can be installed on top of it. The runner image is an example, not a published image.
- `reflect-cronjob.yaml`: `crystalline reflect run --runner k8s` every 5 minutes, with `concurrencyPolicy: Forbid` and a grace period of 30 seconds. `CRYSTALLINE_REMOTE_TOKEN` and the Anthropic credential come from a Secret by reference. Nothing due means the pod ends within seconds.
- `evolve-cronjob.yaml`: the same shape, weekly, with its own account.
- `watch-deployment.yaml`: one replica of `crystalline reflect watch`, for answers within a minute instead of within 5 minutes.

[`examples/github/crystalline-loops.yml`](../examples/github/crystalline-loops.yml) runs the same image on a GitHub Actions schedule, for a team without a cluster. GitHub may start a scheduled job some minutes late.

The evolve loop can also run on an instance of its own instead of in remote mode. The pod starts its own daemon and joins the team domain from its GitHub origin with `CRYSTALLINE_DOMAIN_<NAME>`, `CRYSTALLINE_DOMAIN_<NAME>_ORIGIN` and `CRYSTALLINE_GITHUB_TOKEN` (see [Configure through environment variables](deployment.md#configure-through-environment-variables)), runs `crystalline evolve run`, shares as a pull request and ends. Give every such pod the same `CRYSTALLINE_SERVICE_PUBLIC_URL`, so its first run claims the domain's evolve home and every later pod is that home too. Crystalline never calls that address for the home, it only compares it. A pod without one runs only while the domain has no `evolve_home`. The reflect loop cannot run that way, since the Reflection lives on its home.

## Triggers

A trigger is off by default. Two MANIFEST keys turn it on per domain: `reflect_trigger` and `evolve_trigger`, each `off` or `webhook`. The domain owner changes them, in the MANIFEST or in the domain policies card in Fluid.

Where the webhook goes is the instance's choice, never the MANIFEST's, since a MANIFEST travels with a shared domain. There is one URL for both loops and every domain; the body says which loop and which domain:

| Setting | Variable | Meaning |
|---|---|---|
| `automation.webhook.url` | `CRYSTALLINE_AUTOMATION_WEBHOOK_URL` | Where the webhook is posted, an `http://` or `https://` url. |
| `automation.webhook.secret` | `CRYSTALLINE_AUTOMATION_WEBHOOK_SECRET` (or `_FILE`) | The secret the body is signed with. Shown only as `(set)`. |
| `automation.evolve_threshold` | `CRYSTALLINE_AUTOMATION_EVOLVE_THRESHOLD` | How many mechanical findings make the evolve webhook go out (default 20, minimum 1). |

The three settings are for the operator only: `crystalline config set` or the environment, never an agent through `configure`. A URL without a secret never sends, and status says so: nothing unsigned leaves the instance.

Only the instance that may run the loop sends: the Reflection home for reflect, for evolve the evolve home of a team domain, the host lock holder on a shared database, and any instance with its own copy of the domain otherwise. Nothing is sent while the lease is held.

When it sends:

- Reflect, people waiting (a thread for the agent, feedback): within 60 seconds of the first event, so what arrives in that window is one webhook. It sends again on new events once the 5-minute cap below has passed.
- Reflect, page updates only: after 30 minutes without a further source change on that domain, so a busy day or a bulk import sends one webhook, not hundreds. The quiet clock is per domain.
- Evolve: when the mechanical count reaches `automation.evolve_threshold`, once per crossing. A weekly schedule in the runner stays the normal case.
- When a lease is released and work is still due: within a minute (for evolve the threshold still applies). This is how a thread that arrived during a run gets its own run.

At most one webhook per domain and loop per 5 minutes, and that cap wins over everything above. A failed attempt is tried again after 10, 30 and 90 seconds, unless a run took the lease in the meantime. A delivery that failed every attempt is sent again while the work is due, after 5, 10, 20, 40 and then every 60 minutes. A delivery that gets through starts the list over.

The delivery state ("last sent", "last error") shows in `status` and under the trigger rows of the policies card. It is kept in memory on each instance, so a restart clears it.

### The webhook

```http
POST <automation.webhook.url>
Content-Type: application/json
X-Crystalline-Event: reflect
X-Crystalline-Timestamp: 1792648800
X-Crystalline-Signature: sha256=<hex>

{"counts":{"feedback":1,"pages_to_update":0,"updates":0,"waiting_for_the_agent":2},"domain":"eng","event":"reflect","instance":"crystalline.example.com","sent_at":"2026-10-22T06:00:00Z"}
```

The body carries counts only, never content or paths. `event` is `reflect` or `evolve`; evolve counts are `mechanical` and `judgment`. The signature is the lowercase hex HMAC-SHA256, under the secret, of the timestamp, a dot and the raw body. The timestamp is unix seconds and is part of what is signed, so a receiver that refuses an old one refuses a replay.

Check it on the raw bytes, not on a body parsed and written out again. In a shell, to check a webhook by hand:

```sh
now=$(date +%s)
age=$(( now > timestamp ? now - timestamp : timestamp - now ))
expected="sha256=$(printf '%s.%s' "$timestamp" "$body" | openssl dgst -sha256 -hmac "$secret" | sed 's/^.* //')"
if [ "$age" -le 300 ] && [ "$expected" = "$signature" ]; then echo "signed by the instance"; fi
```

A receiver in code compares in constant time, as the n8n Code node below does.

In an n8n Code node after a Webhook node with the option Raw Body on (the n8n templates do this):

```js
const crypto = require('crypto');
const headers = $input.first().json.headers || {};
const timestamp = String(headers['x-crystalline-timestamp'] || '');
const signature = String(headers['x-crystalline-signature'] || '');
// A missing or old timestamp fails: NaN is not <= 300.
const age = Math.abs(Date.now() / 1000 - Number(timestamp));
if (!timestamp || !(age <= 300)) {
  throw new Error('The webhook has no timestamp or is older than 5 minutes.');
}
const body = await this.helpers.getBinaryDataBuffer(0, 'data');
const expected = 'sha256=' + crypto.createHmac('sha256', $env.CRYSTALLINE_WEBHOOK_SECRET)
  .update(timestamp + '.')
  .update(body)
  .digest('hex');
const given = Buffer.from(signature);
const wanted = Buffer.from(expected);
if (given.length !== wanted.length || !crypto.timingSafeEqual(given, wanted)) {
  throw new Error('The webhook signature does not match.');
}
```

n8n allows `require('crypto')` only with `NODE_FUNCTION_ALLOW_BUILTIN=crypto`, and `$env` only with `N8N_BLOCK_ENV_ACCESS_IN_NODE=false`.

## In Fluid

The Reflection home shows "Last reflected" with the time, the runner and the outcome. It also shows how many items wait for a person. The Maintenance screen shows "Last maintained" per domain and the judgment findings waiting. Nothing is shown when a loop never ran.

What you see depends on your rights. A reader sees the last run without its summary and its pull request. Someone who may run the loop also sees the summary, the pull request and the counts. A Reflection contributor without the right to the knowledge sees no "Last reflected" line. The webhook state shows only for a domain manager or an admin.
