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
latest release, upload the `crystalline-intelligence` skill zip and never open a
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
and ask.* The scaffold arrives as a stub -
its routing sections are what an agent reads each session to decide whether a task
belongs here:

```markdown
---
type: manifest
title: ship-ops
permalink: manifest
tags:
  - manifest
status: stable
recorded_at: 2026-07-19
---

# ship-ops

## Scope

- Describe the knowledge this domain covers

## When to Use

- Describe when an agent should route here

## Notes for Agents

- Add guidance for agents working in this domain
- Note the folder layout new engrams should reuse
```

The placeholders are not yours to type either - describe the domain and let the
agent write its own routing:

```text
Fill in the ship-ops manifest: this domain covers the ship's hardware and
operations - docking gear, coolant loop, vent drivers and hull - and any
question about those systems should route here.
```

*The agent rewrites the placeholder sections in place:*

```markdown
## Scope

- Hardware and operations of the ship: docking gear, coolant loop, vent
  drivers and hull

## When to Use

- Route questions about docking, clamps, coolant or vents here
- Check here before touching hardware the ship has already complained about
```

From then on every session opens with the agent reading
those bullets as a routing brief, so it knows `ship-ops` owns them without being
told. A healthy archive keeps its sync ratio: every engram the agent can see is a
file on disk and every engram file is in the index, and everything below drifts
them apart then back. One kind of file sits outside that count by design - the
`index.md` Crystalline writes into each folder, a plain markdown table of
contents that makes the domain navigable without any tooling. It is generated,
never indexed and never an engram, so it costs the ratio nothing.

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
status: stable
recorded_at: 2026-07-19
generated: { by: claude-code/1.0.5, at: 2026-07-19T09:12:00+00:00 }
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
guidance, not a fixed enum. The stardate `recorded_at` and the `generated`
block, which records who wrote the engram and when, are filled in for you.

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
status: stable
recorded_at: 2026-07-19
generated: { by: claude-code/1.0.5, at: 2026-07-19T11:47:00+00:00 }
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
status: stable
recorded_at: 2026-07-19
generated: { by: claude-code/1.0.5, at: 2026-07-19T14:03:00+00:00 }
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
status: stable
recorded_at: 2026-07-19
source_date: 2026-06-30
resource: https://vendor.example/notices/qx-114
generated: { by: claude-code/1.0.5, at: 2026-07-19T16:20:00+00:00 }
---

# Hyperdrive motivator recall QX-114

The vendor recalls QX-114 motivators shipped before mid-2026. Ours is affected.

## Observations

- [risk] QX-114 motivators built before 2026-06 can desync under sustained load; the vendor offers a free swap #hyperdrive
- [fact] Our unit shipped 2026-04, inside the recall window #hyperdrive
```

Forty screens became two facts and a source link. That is distilling, not
mirroring.

One shape earns verbatim retention, and only alongside its distillation, never
instead of it: a transcript or a long document whose exact words you may need to
quote later. The summary stays an ordinary engram where its topic already lives,
the full text goes in as a `type: source` engram under a `sources/` folder, the
two are linked both ways with `summarizes` and `summarized_by`, and a full text
over the verify token budget (2500 tokens by default) becomes sequential part
engrams. The sin was never keeping the words; it is mirroring instead of
distilling.

A second pass over the vendor's install guide lands another engram, and you tag
that one `hyper-drive` out of habit - a drift the next chapter cleans up. The
other intake jobs follow the same shape, each a sentence you say:

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
nothing changed, that is one frontmatter field, `verified: { by, at }`, kept
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
with `status: stable`, carrying the lesson forward so it outlives the retired
fact:

```markdown
---
type: decision
title: Coolant loop runs glycol mix C
permalink: coolant-loop-runs-glycol-mix-c
tags:
- coolant
- cooling
status: stable
recorded_at: 2026-08-01
generated: { by: claude-code/1.0.5, at: 2026-08-01T17:40:00+00:00 }
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
status: stable
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

Not every retirement has a successor. When a practice is simply abandoned - the
tool is gone, the caveat stopped applying - there is no new engram to write: flip
the old one's status to a retirement value, close its window when you know the
date and carry any lesson worth keeping into a live engram. The retirement is the
whole edit.

The status words each mean one thing: `deprecated` says do not do this again,
`superseded` says a newer engram replaced this one, `archived` says retired but
kept for the record and `legacy` says still deployed and true of old
installations but not to be built on. `delete` is for mistakes, not history.

## Evolve

Every chapter so far began with you noticing something: a clamp that misreads, a
mix that ran hot, a tag you typed two ways. That works while you still remember
what you touched. A few weeks into the run the archive has outgrown your memory
of it, and the useful question turns around. Instead of telling the ship what
changed, you ask the ship what it needs:

```text
Sweep ship-ops and tell me what the archive needs - what has gone stale, what is
half-finished, what looks duplicated.
```

*The agent runs a read-only sweep of the whole domain. Nothing is written: what
comes back is a ranked queue where every item carries the evidence it fired on
and the one action that clears it.*

```text
Sweep of ship-ops as of 2026-09-14
23 engrams scanned, 6 findings (showing 6, page 1)
temporal 3, structure 2, redundancy 1

1. [90] V005 MECHANICAL
   Hyperdrive motivator recall QX-114  crystalline://ship-ops/hyperdrive-motivator-recall-qx-114
   still stable but already superseded by ship-ops/hyperdrive-motivator-swap-qx-114b
   evidence: status=stable; superseded by ship-ops/hyperdrive-motivator-swap-qx-114b; inbound refs 2
   fix: set_frontmatter status=superseded

2. [85] V001 JUDGMENT
   Bay 3 clamp bypass  crystalline://ship-ops/bay-3-clamp-bypass
   validity window ended 2026-08-31 but the status is still stable
   evidence: valid_to=2026-08-31; today=2026-09-14; status=stable; inbound refs 0
   fix: set_frontmatter valid_to=<later date> or status=superseded

3. [70] V002 JUDGMENT
   Vent Driver Firmware  crystalline://ship-ops/vent-driver-firmware
   stale since 2026-09-01 with no verification recorded since
   evidence: stale_after=2026-09-01; today=2026-09-14; never verified
   fix: set_frontmatter verified=<actor> or stale_after=<later date>

4. [50] V102 MECHANICAL
   Cold-bay docking checklist  crystalline://ship-ops/cold-bay-docking-checklist
   unresolved reference [[Vent Drive Firmware]]
   evidence: rel_type=depends_on; nothing titled `Vent Drive Firmware` in ship-ops; nearest is `Vent Driver Firmware`
   fix: [[Vent Drive Firmware]] -> [[Vent Driver Firmware]]

5. [35] V103 MECHANICAL
   Hyperdrive install summary  crystalline://ship-ops/hyperdrive-install-summary
   ship-ops/sources/qx-114-install-guide declares summarizes but the summarized_by back-link is missing
   evidence: ship-ops/sources/qx-114-install-guide -summarizes-> ship-ops/hyperdrive-install-summary; no summarized_by pointing back
   fix: append `- summarized_by [[QX-114 install guide]]`

6. [30] V203 JUDGMENT
   crystalline://ship-ops
   2 tag spellings look like one tag (plural variants)
   evidence: #vent used 1 time(s); #vents used 5 time(s)
   fix: crystalline tags merge vent vents
```

That is the queue itself. Below it the sweep prints an `Actions:` legend, one
line per rule spelling out the prescribed fix in full, any `Truncated:` note
where a rule capped its own output and one standing reminder that the queue
changes nothing by itself. A clean domain prints `nothing to work in this
scope`, which is a perfectly good answer to have asked for.

The number in brackets is the priority: how much the ship gains by fixing this
one first. The word beside it is the one that decides who acts. `MECHANICAL`
means the finding completes intent the archive already records - the swap engram
already declares it supersedes the recall, so flipping the recall's status
invents nothing. `JUDGMENT` means acting would change what the archive claims,
and that is yours to approve. So the work splits in two, and the agent handles
each half differently:

```text
Do the mechanical ones.
```

*Three items, one pass, one summary at the end - no item-by-item permission for
work that only finishes what was already decided.* The recall gets
`status: superseded` and a `- superseded_by` relation, the misspelled link
becomes `[[Vent Driver Firmware]]` and the summary gets its `- summarized_by`
back-link. The queue is not a script, though, so the agent still reports what it
did rather than going quiet.

The other three arrive one at a time, each as a proposal:

```text
The bay 3 clamp bypass expired on 31 August. Bay 3 was re-shimmed on the 12th,
so I read this as ended rather than extended: retire it, close the window on the
12th and carry the shim lesson onto the clamp engram? (y / n)
```

You say yes, and only then does it write. The firmware one you send away: the
sweep says nobody has confirmed the shared-image fact since the review date came
up, and confirming it means walking to the vent bay, so it stays in the queue
until you have. The tag drift comes with a command rather than an edit, because
folding tags is a bulk rewrite the agent never does on its own: it hands you
`crystalline tags merge vent vents` for the
[terminal corner](#the-terminal-corner) and leaves it there.

Two things are missing from that queue, and both are the point. The
`hyper-drive` spelling you folded back in Reconcile is gone for good: the sweep
reads the `## Tag Aliases` section that the merge wrote into the MANIFEST, so a
fold done once never comes back as a finding. And the glycol mix B retirement
from Retire is not there either - you did all three steps that day, so there is
nothing left to find. Item 1 is simply what that same edit looks like when a
shift ends between step two and step three.

Which is the honest limit of the whole thing. The sweep reads dates, links and
graph shape: a status, a validity window, an edge that resolves or does not, a
tag spelled two ways. It never reads for meaning. It found item 1 because a
`supersedes` edge points at an engram still marked stable, not because it
compared the recall with the swap and formed a view about which one is right. It
will hand you a half-finished retirement every time and it will never tell you
that two engrams contradict each other - even the duplicate detection is
lexical, so two engrams saying the same thing in different words stay invisible
to it. It finds the work; you still decide it.

When the queue is worked, ask again:

```text
Run that sweep again.
```

*Same scope, same day, and the sweep re-derives the answer from scratch - nothing
about the first run was stored, so "what is left" is only ever what is still
true:*

```text
Sweep of ship-ops as of 2026-09-14
23 engrams scanned, 2 findings (showing 2, page 1)
temporal 1, redundancy 1
```

Six down to two, and the two still standing are the firmware re-check and the tag
merge: both are waiting on you rather than on the archive, one for a walk to the
vent bay and one for a command only you should run. That shrinking number is the
whole feedback loop: it is how you
know the pass landed, and it is why the sweep is worth running on a cadence
rather than once. Ask for it after a big ingest drops a dozen engrams at once,
when two search hits disagree and a half-finished retirement is the likely
reason, or on the first shift of a month. Leave the domain out and it sweeps
everything you have; ask for one family and it sticks to the temporal, structural
or redundancy findings alone. What it is not is a session-start ritual: this is
deliberate maintenance you ask for, never a chore the ship nags you about.

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
conflict, keeping your side or theirs.

Review is a conversation, not a verdict. When command asks for changes:

```text
What did the review say about my fleet-ops proposal?
```

*The agent pulls the domain, relays the reviewers' comments, and after you refine
the engrams, sharing again updates the same proposal - same number, same review
URL - so the conversation stays in one place.* A declined proposal is normal;
ask the agent to withdraw it, which closes it on GitHub and tidies the record,
optionally restoring the shared files. Hard-won knowledge is worth the review.

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
| Ask what needs work | "Sweep ship-ops and tell me what the archive needs." |
| Share with the team | "Share the clamp findings as a proposal for review." |
| Withdraw a proposal | "Withdraw the fleet-ops proposal and keep my local edits." |

### Reference blocks

Recommended `status` values (guidance, not enforced): `stable`, `implemented`,
`draft`, `proposed`, `idea`, `poc`, `deprecated`, `superseded`, `archived`,
`legacy`. `stable` is the default a write fills in and the Open Knowledge Format
word for knowledge that holds now; `current` is the older Crystalline word for
the same state, still written by hand and read forever, and a search filtered on
either word returns engrams carrying either.

Observation categories: `- [decision]`, `- [fact]`, `- [pattern]`, `-
[gotcha]`, `- [convention]`, `- [lesson]`, `- [risk]`, `- [insight]`, `-
[idea]`, `- [proposal]` and `- [poc]`. Free text, so reach for the most precise
one.

Relation syntax: `- rel_type [[Other Engram]]`, or quote a multi-word type,
`- "relates to" [[Other Engram]]`. Aliases in a MANIFEST are `- old -> canonical`.

Temporal rules: no `valid_from` means always valid, no `valid_to` means valid
forever. `stale_after` (legacy spelling `review_after`) says when the knowledge
is due a re-check, and `verified: { by, at }` records who last checked it. Set a bound only when validity is genuinely limited, as a plain ISO date
(`YYYY-MM-DD`). Never write a sentinel far-future date to mean forever - absence
already means it.

Address scheme: `crystalline://<domain>/<permalink>` is the one absolute form. Any
identifier without the scheme is domain-relative, so pass a bare permalink and
name the domain separately.

### The terminal corner

Optional power-user territory - a reader who never opens a terminal can skip it.
But like the quarantine protocol on a certain other freighter, the checks earn
their keep: run `verify` and `doctor` before you trust a shared branch. Most of
these are commands the chapters never handed to the agent:

```sh
crystalline install claude-code                     # wire up a harness (Setup)
crystalline import ./old-notes --domain ship-ops    # convert a legacy markdown tree
crystalline tags rename <old> <new>                 # rename a tag everywhere
crystalline tags merge <old> <into>                 # fold one tag into another
crystalline origin withdraw fleet-ops --proposal 4  # close and clear a proposal
crystalline verify                                  # static check: frontmatter, links, schema
crystalline doctor                                  # diagnose index and service; add --fix to repair
crystalline evolve --domain ship-ops                # the Evolve sweep, run by hand
crystalline reindex --full                          # rebuild the derived index from the files
```

The index is disposable and the files are the truth, so `reindex --full` is never
a data-loss event - it is the clean-room reset that syncs the index back to the
engram files, ratio restored (the generated `index.md` files stay outside it, as
they always are).

---

That is the whole flight: record what you learn, query it back, ingest by
distilling, reconcile in place, retire without forgetting, sweep for what the
archive needs and share for review.
Keep the log honest and the ship stops being a stranger to itself. See you around,
space cowboy.
