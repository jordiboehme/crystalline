```
          ·                        *
              ▄▄▄████▄▄▄
            ▟███▓▒░░▒▓███▙            T H E   C R Y S T A L L I N E
             ▜███▓▒░▒▓███▛                · ·  P L A Y B O O K  · ·
        *      ▀▜███▛▀                    a field manual, in use-cases
                  ▀
```

# The Crystalline Playbook

You are the new knowledge officer aboard the SS Kobayashi Delorean, a small
freighter that hauls whatever pays - flux capacitors one run, live cargo from an
island job the next. Your crew includes an AI agent that starts every shift a
stranger and forgets the last one, unless you teach it to remember. This is a
use-case course, and one dataset threads through all of it: what you record in
`Record` is what you query, reconcile, retire and share later. Follow it in order
and the ship ends up keeping its own log.

Every step is a conversation. You speak in plain terms; the agent infers the rest,
proposes and does it - the italic line after each prompt shows that inference at
work, and each chapter also shows the artifact that lands on disk so you can see
exactly what the agent wrote. Only setup and a small terminal corner touch a
command line. The [reference README](../README.md) is the map; this is the flight
training.

## Setup

Install the binary once - pick your platform from the
[install matrix](../README.md#install-the-binary) and come back. Then one command
wires the whole integration:

```sh
crystalline install claude-code
```

That single step registers the MCP server, the `SessionStart` routing hook, the
`Stop` capture nudge and the four skills. Think of it as the agent loading a
program: next session it wakes already knowing how to route, capture and share.
The same command takes `codex` or `copilot`. This is the last time you need a
terminal. Claude Desktop skips even this: install the `.mcpb` extension from the
latest release, upload the `crystalline-memory` skill zip and never open a
terminal.

Start a session and give the agent its first domain in plain language:

```text
Create a new Crystalline domain called ship-ops for everything about this ship -
the docking gear, the coolant loop, the vent drivers.
```

*The agent creates a file domain named `ship-ops` under its domains root
(`~/Documents/Crystalline` by default), scaffolds its `MANIFEST.md` and registers
it, confirming the location before it writes anything. Naming the domain yourself
keeps the outcome predictable; leave the name out and the agent will propose one
and ask.* The scaffold is a stub you fill in -
its routing sections are what an agent reads each session to decide whether a task
belongs here:

```markdown
---
type: manifest
title: ship-ops
permalink: manifest
tags:
  - manifest
status: current
recorded_at: 2026-07-19
---

# ship-ops

## Scope

- Describe the knowledge this domain covers

## When to Use

- Describe when an agent should route here

## Notes for Agents

- Add guidance for agents working in this domain
```

Replace the placeholders with the ship's real scope - docking, coolant, the vent
drivers and the hull - so the `When to Use` bullets route clamp and coolant
questions straight here. From then on every session opens with the agent reading
those bullets as a routing brief, so it knows `ship-ops` owns them without being
told. A healthy archive keeps its sync ratio: what the agent sees and what is on
disk track one to one, and everything below drifts them apart then back.

## Record

Capture is a byproduct of the work, not a chore. Say what you learned the way you
would tell a crewmate:

```text
Remember this: docking clamp 3 reads locked about half a second before it
actually seats when the bay is cold, so wait for the green tone before cutting
thrust.
```

*You named no domain and no category. The agent searches `ship-ops` first, finds
nothing, then proposes filing it as a `[gotcha]` tagged `docking` and writes only
once you say yes - it named the domain for you, so nothing lands wrong.* Here is
the artifact that lands on disk:

```markdown
---
type: engram
title: Docking clamp cold-weather seating
permalink: docking-clamp-cold-weather-seating
tags:
- docking
- clamps
status: current
recorded_at: 2026-07-19
timestamp: 2026-07-19T09:12:00+00:00
---

# Docking clamp cold-weather seating

Clamp 3 misreports its lock state when the aft bay is cold.

## Observations

- [gotcha] Clamp 3 reads locked about half a second before it seats; wait for the green tone before cutting thrust #docking
- [fact] Seen below roughly 5C in the aft bay #docking
```

The top-level `- [category] text #tag` bullets are observations, the atomic unit
of an engram. Categories are free text but precise ones earn their keep (the
appendix lists them); `type` and `status` have recommended values that are
guidance, not a fixed enum. The stardate `recorded_at` and the `timestamp` are
filled in for you.

The agent proposes before it captures. Even unprompted it would raise the insight
first - "I noticed clamp 3 misreads in the cold; should I record that in
`ship-ops`?" - and wait for your yes. A blunt "store this" is already a complete
instruction; idle grumbling about the clamp it lets pass without a word.

Next you record how the vents are driven, a fact other knowledge will lean on:

```text
Note that every vent actuator runs one shared firmware image, flashed from a
single controller. A bad flash grounds all the vents at once.
```

*The agent files it with `type: reference`, catching the single-point-of-failure
angle as a `[risk]`:*

```markdown
---
type: reference
title: Vent Driver Firmware
permalink: vent-driver-firmware
tags:
- vents
- firmware
status: current
recorded_at: 2026-07-19
timestamp: 2026-07-19T11:47:00+00:00
---

# Vent Driver Firmware

Every vent actuator runs one shared firmware image, flashed from a single
controller.

## Observations

- [fact] All vent actuators run the same firmware build from one controller #vents
- [risk] The shared image is a single point of failure; a bad flash grounds every vent at once #vents
```

Now knowledge that leans on other knowledge gets a relation. You still just talk:

```text
Also note we decided to run the coolant loop on glycol mix B - it rides on the
vent driver firmware.
```

*The agent files a `[decision]` and, seeing the Vent Driver Firmware engram it
just wrote, links the two.* On disk the decision carries a `Relations` section:

```markdown
---
type: decision
title: Coolant loop runs glycol mix B
permalink: coolant-loop-runs-glycol-mix-b
tags:
- coolant
- cooling
status: current
recorded_at: 2026-07-19
timestamp: 2026-07-19T14:03:00+00:00
---

# Coolant loop runs glycol mix B

The primary coolant loop runs glycol mix B for its cold-start margin.

## Observations

- [decision] Run the coolant loop on glycol mix B #coolant
- [fact] Chosen for a wider cold-start window than the stock coolant #coolant

## Relations

- depends_on [[Vent Driver Firmware]]
```

The `[[Vent Driver Firmware]]` link resolves to the engram you just recorded, so
the graph knows the coolant choice rides on the firmware. One more capture goes in
the same way: on the walkaround you notice the two-meter thermal exhaust port
vents straight to the reactor with no shielding, and the agent files it as a
`[risk]` tagged `structural`. Four engrams recorded now. Everything after this
queries, refines or retires them.

## Query

Ask the way the question forms in your head. A narrow ask stays in one domain:

```text
What do we know about the docking clamps?
```

*A narrow question, so the agent scopes itself to `ship-ops` and relays the hit
with its tags visible:*

```text
1 hit in ship-ops - Docking clamp cold-weather seating [docking, clamps]
  - [gotcha] Clamp 3 reads locked about half a second before it seats; wait for
    the green tone before cutting thrust
```

A broad ask drops the domain filter and sweeps everything you have - just
`ship-ops` today, fanning out across every domain as the archive grows. The net is
vast:

```text
Any single points of failure we should worry about?
```

*The agent sweeps every domain and relays the two engrams that carry a
single-point-of-failure `[risk]`, tags attached:*

```text
2 hits in ship-ops
  Vent Driver Firmware                 [vents, firmware]
  Thermal exhaust port is unshielded   [structural]
```

Both hits earned their place: the shared vent firmware grounds every vent on a bad
flash, and the exhaust-port `[risk]` you filed on the walkaround vents straight to
the reactor. The clamp gotcha is not here - it is annoying, not fatal, so it does
not match the question. For the neighbourhood around one engram rather than a flat
list, hand the agent a `crystalline://` anchor - "walk out from
`crystalline://ship-ops/coolant-loop-runs-glycol-mix-b`" - and it follows the
relations to the vent driver firmware and back.

Time matters too. Ask *what is true now?* and the agent filters on status; ask
*what coolant applied last June?* and it reasons over validity windows instead.
Mind the spoilers: an engram with no `valid_from` has always been valid and one
with no `valid_to` is valid forever, so a strict date bound sees only the windows
that exist and can skip the unbounded rows - the very ones the question means to
keep. Absence means always, so the agent leans on status for now-versus-then.

Coming back after a few shifts away, ask "what changed while I was away?" and the
agent pulls the recent captures across every domain so you start current. An
engram is what persists of a session after the session is gone; the more you
record, the more the ship recalls.

## Ingest

Crystalline ships no scraper, by design: the agent is the ingester. It reads a
source and distills it into engrams, one truth per domain. The cardinal sin is
mirroring - copying a source wholesale so it looks complete, then filling the gaps
with guesswork the way the old park bred its exhibits with spare frog DNA. Distill
the durable facts and drop the rest.

Make the contrast concrete. A vendor recall notice for the hyperdrive is a
forty-screen webpage: marketing header, a revision table, legalese, a support
matrix and three paragraphs that actually matter. You say:

```text
Read this vendor recall page and remember only the parts that affect our ship -
keep a link back to the source.
```

*The agent distills the durable risk into `ship-ops`, records the source in
frontmatter and drops the rest:*

```markdown
---
type: reference
title: Hyperdrive motivator recall QX-114
permalink: hyperdrive-motivator-recall-qx-114
tags:
- hyperdrive
- vendor
- recall
status: current
recorded_at: 2026-07-19
source_date: 2026-06-30
resource: https://vendor.example/notices/qx-114
timestamp: 2026-07-19T16:20:00+00:00
---

# Hyperdrive motivator recall QX-114

The vendor recalls QX-114 motivators shipped before mid-2026. Ours is affected.

## Observations

- [risk] QX-114 motivators built before 2026-06 can desync under sustained load; the vendor offers a free swap #hyperdrive
- [fact] Our unit shipped 2026-04, inside the recall window #hyperdrive
```

Forty screens became two facts and a source link. That is distilling, not
mirroring. A second pass over the vendor's install guide lands another engram, and
you tag that one `hyper-drive` out of habit - a drift the next chapter cleans up.
The other intake jobs follow the same shape, each a sentence you say:

- **Local documents.** "Read the raptor-paddock containment PDFs in `./isla-nublar`
  and propose engrams for a new `containment` domain, the keepers only." The agent
  reads the files and proposes an engram list before writing a line.
- **A git repository.** "Look through the flux-capacitor firmware repo and distill
  how it is built and its conventions into a new `vessel-arch` domain." The agent
  proposes the domain and the engram list first, holds for your yes, then captures
  distilled architecture - never a copy of the source.
- **Your team wiki.** "Export our team wiki to markdown and bring over the pages
  still worth keeping, leaving the dead ones behind." It distills the keepers and
  leaves the fossils where they lie.

Proposing first is the rule for a whole domain too. Point the agent at the
flux-capacitor firmware repo and it comes back with a list before touching disk:

```text
Proposed for a new vessel-arch domain (5 engrams):
  - Flux capacitor firmware architecture
  - Build and flash workflow
  - Timing-calibration convention
  - Fault codes and recovery
  - Hardware revision notes
Write these? (y / n)
```

You trim it to the ones that matter and it writes only those, distilled into their
own domain rather than dumped into `ship-ops`.

A legacy markdown tree that is already frontmatter-shaped is the one exception
that stays in the terminal: an import command in the
[terminal corner](#the-terminal-corner) folds it into a domain, source tree
untouched. They spared no expense on that old archive, and it shows - half of it
is scaffold. Distill.

## Reconcile

Two standing orders keep the record single: search before you write, and edit over
create. A later cold snap teaches you the clamp misread starts earlier than you
first logged - not at 5C but nearer 8C. You do not file a second clamp engram:

```text
Correction on the docking clamps: the cold misread starts around 8C, not 5C. We
measured it more carefully this run.
```

*The agent searches `ship-ops` first, finds the existing gotcha and refines it in
place rather than forking a duplicate.* This is a correction, so it edits the fact
where it stands - the observation section goes from

```markdown
- [gotcha] Clamp 3 reads locked about half a second before it seats; wait for the green tone before cutting thrust #docking
- [fact] Seen below roughly 5C in the aft bay #docking
```

to

```markdown
- [gotcha] Clamp 3 reads locked about half a second before it seats; wait for the green tone before cutting thrust #docking
- [fact] Seen below roughly 8C in the aft bay #docking
```

An engram is what is true now, so a changed value is replaced where it stands, not
stapled on as a dated `## Update` note. When you have only re-checked a source and
nothing changed, that is one frontmatter field, `last_verified: <date>`, kept
current - never a heading.

The test is simple: does the new fact make the old one false going forward? A
sharper measurement of the same behavior does not, so it is a correction edited in
place. A value that genuinely changed in the world does, and that is a
supersession - the next chapter.

Vocabulary drifts the same way. Two engrams now touch the hyperdrive - the recall
notice tagged `hyperdrive` and the install guide tagged `hyper-drive` - the same
topic split under two spellings. The agent surveys the vocabulary and surfaces the
pair as a near-duplicate cluster rather than acting on its own. Folding them is a
deliberate bulk rewrite, so the merge lives in the
[terminal corner](#the-terminal-corner). It rewrites every tag and records the
fold in the domain MANIFEST so nothing gets lost:

```markdown
## Tag Aliases

- hyper-drive -> hyperdrive
```

From then on a search for `hyper-drive` folds into `hyperdrive` in both
directions, so the old name keeps finding everything it always did.

## Retire

Knowledge retires, it does not disappear - all of it stays in the record so the
crew never re-learns it the hard way. When mix B gives way to mix C, you supersede
rather than overwrite:

```text
We switched the coolant to glycol mix C on 2026-08-01 - mix B ran hot above 80%
load. Retire the old decision but keep why it changed.
```

*The agent runs the full recipe.* First it writes the replacement as a new engram
with `status: current`, carrying the lesson forward so it outlives the retired
fact:

```markdown
---
type: decision
title: Coolant loop runs glycol mix C
permalink: coolant-loop-runs-glycol-mix-c
tags:
- coolant
- cooling
status: current
recorded_at: 2026-08-01
timestamp: 2026-08-01T17:40:00+00:00
---

# Coolant loop runs glycol mix C

The primary coolant loop runs glycol mix C after mix B overheated under load.

## Observations

- [decision] Run the coolant loop on glycol mix C #coolant
- [lesson] Mix B ran hot above 80% load; mix C holds its margin there #coolant

## Relations

- supersedes [[Coolant loop runs glycol mix B]]
```

Then it edits the old engram - flipping its status and closing its validity window
in one edit, and adding the back-relation. The old frontmatter goes from

```markdown
status: current
```

to

```markdown
status: superseded
valid_to: 2026-08-01
```

with a `- superseded_by [[Coolant loop runs glycol mix C]]` line added to its
relations. The old decision is still readable and still addressable by date, but it
can no longer read as current. Use the real transition date for `valid_to`, never
a sentinel, and leave the window open when the date is unknown.

The status words each mean one thing: `deprecated` says do not do this again,
`superseded` says a newer engram replaced this one and `archived` says retired but
kept for the record. `delete` is for mistakes, not history.

## Share

A team domain is an ordinary domain whose files also live in a GitHub repository:
your local markdown stays the truth, and an origin records which repository it
tracks. Wiring up the fleet is a conversation, not a config file:

```text
Turn on GitHub team sharing, connect this machine, then pull in the fleet's shared
repo fleet/fleet-ops.
```

*The agent turns on team sharing and hands you a short browser code to confirm - no
git, no SSH keys - then registers `fleet-ops` as a team domain tracking that
repository's main branch and downloads it.*

The loop is a rhythm you speak. Ask where the domain stands at session start, pull
the team's merged work before you dig in and share when your own work is worth it:

```text
Share my docking clamp findings to fleet-ops as a proposal and give me the review
URL.
```

*The agent opens a proposal from your local changes and hands back a review URL.* A
person at command reads it and merges on GitHub; the agent never merges its own
work, it only relays the link. If two edits collide, ask the agent to resolve the
conflict, keeping your side or theirs. A declined proposal is normal - it lapses,
or you discard it from the [terminal corner](#the-terminal-corner). Hard-won
knowledge is worth the review.

## Appendix

### Quick reference

| To do this | Say to your agent |
|---|---|
| Commission a domain | "Create a new Crystalline domain called ship-ops for everything about the ship." |
| Capture a fact | "Remember this: the port clamp sticks in the cold." |
| Recall, scoped | "What do we know about docking?" |
| Recall, everywhere | "Any single points of failure we should worry about?" |
| Walk the graph | "Walk out from the coolant decision and show what connects." |
| Recall what was true then | "What coolant applied last June?" |
| Catch up | "What changed while I was away?" |
| Ingest a source | "Read this recall page and remember only what affects us." |
| Correct a fact | "Update the clamp threshold, do not start a new note." |
| Retire a fact | "The old coolant mix is retired - supersede it, keep why." |
| Tidy vocabulary | "Have our hyperdrive tags drifted?" |
| Share with the team | "Share the clamp findings as a proposal for review." |

### Reference blocks

Recommended `status` values (guidance, not enforced): `current`, `implemented`,
`draft`, `proposed`, `idea`, `poc`, `deprecated`, `superseded`, `archived`,
`legacy`.

Observation categories: `- [decision]`, `- [fact]`, `- [pattern]`, `-
[gotcha]`, `- [convention]`, `- [lesson]`, `- [risk]`, `- [insight]`, `-
[idea]`, `- [proposal]` and `- [poc]`. Free text, so reach for the most precise
one.

Relation syntax: `- rel_type [[Other Engram]]`, or quote a multi-word type,
`- "relates to" [[Other Engram]]`. Aliases in a MANIFEST are `- old -> canonical`.

Temporal rules: no `valid_from` means always valid, no `valid_to` means valid
forever. Set a bound only when validity is genuinely limited, as a plain ISO date
(`YYYY-MM-DD`). Never write a sentinel far-future date to mean forever - absence
already means it.

Address scheme: `crystalline://<domain>/<permalink>` is the one absolute form. Any
identifier without the scheme is domain-relative, so pass a bare permalink and
name the domain separately.

### The terminal corner

Optional power-user territory - a reader who never opens a terminal can skip it.
But like the quarantine protocol on a certain other freighter, the checks earn
their keep: run `verify` and `doctor` before you trust a shared branch. These are
the only commands the chapters did not hand to the agent:

```sh
crystalline install claude-code                     # wire up a harness (Setup)
crystalline import ./old-notes --domain ship-ops    # convert a legacy markdown tree
crystalline tags rename <old> <new>                 # rename a tag everywhere
crystalline tags merge <old> <into>                 # fold one tag into another
crystalline origin discard fleet-ops --proposal 4   # abandon a declined proposal
crystalline verify                                  # static check: frontmatter, links, schema
crystalline doctor                                  # diagnose index and service; add --fix to repair
crystalline reindex --full                          # rebuild the derived index from the files
```

The index is disposable and the files are the truth, so `reindex --full` is never
a data-loss event - it is the clean-room reset that syncs the index back to the
files, ratio restored.

---

That is the whole flight: record what you learn, query it back, ingest by
distilling, reconcile in place, retire without forgetting and share for review.
Keep the log honest and the ship stops being a stranger to itself. See you around,
space cowboy.
