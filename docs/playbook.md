```
          ·                        *
              ▄▄▄████▄▄▄
            ▟███▓▒░░▒▓███▙            T H E   C R Y S T A L L I N E
             ▜███▓▒░▒▓███▛                · ·  P L A Y B O O K  · ·
        *      ▀▜███▛▀                    a field manual, in missions
                  ▀
```

# The Crystalline Playbook

You are the new knowledge officer aboard the SS Kobayashi Delorean, a small
freighter whose transponder still answers to three older names. Your crew
includes an AI agent that starts every shift a stranger and forgets the last
one, unless you teach it to remember. Eight missions below, each one runnable
start to finish: work through them and by the end the ship keeps its own log,
the agent reads it before it acts and neither of you re-derives the docking
procedure from scratch ever again.

Each mission is agent-first - the natural-language prompt you say out loud -
with the exact CLI equivalent underneath for when you want to drive by hand.
The [reference README](../README.md) is the map; this is the flight training.

## Mission 00 - Preflight

> *Tank, load me the operations manual. A pause. I know the ship.*

Install the binary first. Pick your platform from the
[install matrix](../README.md#install-the-binary) - Homebrew, a `.deb`, an MSI
or a raw binary - and come back. Then one command jacks the agent in:

```sh
crystalline install claude-code
```

That single step wires the whole rig: the MCP server registration, the
`SessionStart` routing hook, the `Stop` capture nudge and the four topical
skills. It is the "I know kung fu" moment - next session the agent wakes up
already knowing how to route a question, capture a lesson and share with a
team, with no upload of your own. The same command takes `codex` or `copilot`.
Claude Desktop has no terminal step: install the `.mcpb` extension from the
latest release and upload the `crystalline-memory` skill zip.

Run the systems check:

```sh
crystalline status
```

Call it the archive's sync ratio: `status` reports how many engrams are indexed
against what is on disk, and a healthy pair tracks one to one. If they drift, the
index is derived and disposable - you will rebuild it in the appendix without
losing a byte of knowledge.

## Mission 01 - Commission the archive

> *Computer: commission the ship's archive. Log our five-year mission.*

A domain is a folder of knowledge with a `MANIFEST.md` at its root. `domain
init` scaffolds the folder and its manifest; `domain add` registers and indexes
it in one step, no separate sync:

```sh
crystalline domain init ~/knowledge/ship-ops --name ship-ops
crystalline domain add ship-ops ~/knowledge/ship-ops
```

Now open that `MANIFEST.md` and write its `## Scope` and `## When to Use`
sections. This is the routing beacon, not marketing copy: the `When to Use`
bullets are exactly what an agent reads at the start of every session to decide
whether a task belongs in `ship-ops` at all. Write them like standing orders -
"when asked about docking, coolant or the vent drivers, look here". Vague scope
is a computer that answers every hail with "insufficient data".

Next session the agent sees the domain in its routing block automatically. You
have given it somewhere to think.

## Mission 02 - First entries

> *Captain's log, supplemental: the docking clamps, again.*

Capture is a byproduct of the work, not a chore saved for the end. The atomic
unit is the observation - a top-level bullet `- [category] a fact or lesson
#tag`. Say it to the agent in plain language and let it do the ceremony:

```text
Capture this in ship-ops as a [gotcha]: docking clamp 3 reports locked about
half a second before it actually seats, so wait for the green tone before you
cut thrust. Tag it docking.
```

The agent searches `ship-ops` first (there is no writing before searching),
finds nothing, proposes the engram and writes it once you say yes. Notice you
named the domain: every write names one, always, so knowledge never lands in
the wrong hold. The CLI equivalent:

```sh
crystalline write ship-ops "Docking clamp 3 seats late" \
  --content "- [gotcha] Clamp 3 reads locked ~0.5s before it seats; wait for the green tone #docking" \
  --tags docking --type engram
```

Knowledge that leans on other knowledge gets a relation. Ask for a decision
that points at an existing engram:

```text
Capture in ship-ops as a [decision]: we route the coolant loop through glycol
mix B. Link it to the vent driver firmware engram.
```

That records a `- depends_on [[Vent Driver Firmware]]` line, and the graph now
knows the coolant choice rides on the firmware. Categories are free text but
precise ones earn their keep - `decision`, `fact`, `pattern`, `gotcha`,
`convention`, `lesson`, `risk`, `insight` and `idea` cover most of a shift.
The `type` and `status` fields have recommended values too, but they are
guidance, not an enum a write is rejected for. Your stardate (`recorded_at`) is
filled in for you.

## Mission 03 - Recall

> *Time is a big ball of what-was-true-then. Do be careful. Spoilers.*

A narrow question searches one domain; a broad one sweeps them all. Ask
narrowly and the agent scopes itself:

```text
What do we know about the docking clamps?
```

Ask broadly and it casts the net wide - the archive is vast, and every domain
answers:

```text
Search everything we know about coolant.
```

The CLI mirrors both, plus filters and the graph walk:

```sh
crystalline search "docking clamps" --domain ship-ops
crystalline search "coolant" --tag docking --status current
crystalline read docking-clamp-3-seats-late --domain ship-ops
crystalline context "crystalline://ship-ops/coolant-loop-routing" --depth 2
```

`--status current` asks for what holds now; the record still remembers what
held then. `--after` matches the recorded date - `recorded_at`, filled in on
every write - so it narrows by when a fact was captured, never by its validity.
Here is the timey-wimey warning, so heed it: a what-was-true-then question runs
on validity windows instead, and an engram with no `valid_from` has always been
valid while one with no `valid_to` is valid forever. A strict validity-date
predicate compares only the bounds that exist, so it can quietly skip those
unbounded rows - the very ones a "what applied last June" query means to keep.
Absence means always, so lean on `status` for now-versus-then and do not filter
the open-ended rows into silence.

To catch up on a return from leave:

```sh
crystalline recent --timeframe 7d
```

An engram is what persists of a session after the session itself is gone. Write
enough of them and the ship remembers what the crew forgets.

## Mission 04 - Bulk intake

> *Legacy knowledge, like life, finds a way. Preserve it in amber, not a tar pit.*

Crystalline ships no scraper, by design. The agent is the ingester: it reads a
source and distills it into engrams, one truth per domain. The cardinal sin is
mirroring - copying a source wholesale so it looks complete. That is filling
the gaps with frog DNA: it breeds surprises you did not sign off on. Distill the
durable facts, drop the rest and keep the domain a clean model of what is true,
not a photocopy of where you found it.

Four intake jobs, four prompts. A webpage (this one needs a harness with web
access):

```text
Read <url> and distill the durable facts into ship-ops. Put the source URL in
resource and its date in source_date, and skip anything that goes stale next
week.
```

Local documents on disk:

```text
Read the three PDFs in ./specs and propose engrams for ship-ops - the keepers
only, one topic each.
```

A git repository, into its own new domain:

```text
Clone <repo>, distill its architecture and conventions into a new domain called
vessel-arch. Propose the engram list first; do not write anything yet.
```

Your team wiki:

```text
Export the wiki pages to markdown in ./wiki-export, distill the pages worth
keeping into ship-ops and leave the fossils in the amber.
```

For a legacy markdown tree that is already frontmatter-shaped, there is one CLI
path that converts it into the domain - normalizing types, backfilling status
and dropping sentinel dates, leaving your source tree untouched:

```sh
crystalline import ./old-notes --domain ship-ops
```

They spared no expense on the old archive, and it shows: half of it is scaffold.
The Jurassic Park lesson holds - the crew got so preoccupied with whether they
could ingest all of it that nobody stopped to ask whether they should. Distill.

## Mission 05 - Reconciliation

> *If my calculations are right, when two engrams disagree you are about to see
> some serious reconciliation.*

Two standing orders keep the timeline single: search before you write, and edit
over create. When new knowledge lands on a topic that already has an owner,
refine the owner in place - do not fork a rival engram that quietly contradicts
it. Remind the agent when it forgets:

```text
Before you write that, search ship-ops - we may already have a coolant engram
to update instead.
```

Reconcile in place, never as an append log. An engram is what is true now, so
you replace the changed fact where it stands; you do not staple a dated
`## Update` section under it. "Checked the source, nothing changed" is not a
heading either - it is one frontmatter field, `last_verified: <date>`, kept
current with a `find_replace`. A stale engram nobody has re-checked is the
photograph where the crew is slowly fading out; a current `last_verified` is
what keeps everyone in the picture.

Vocabulary drifts the same way a timeline does - `docking` here, `docking-clamp`
there, the same idea under two names. Surface it and fold it:

```sh
crystalline vocabulary --domain ship-ops
crystalline tags merge docking-clamp docking --dry-run
crystalline tags merge docking-clamp docking
```

A merge records `- docking-clamp -> docking` in the MANIFEST's `## Tag Aliases`
section, so searches for the old name still find everything - the timeline is
fixed without erasing the past that led there. The agent may propose an alias
when it spots the drift, but it edits that section only after you agree.

## Mission 06 - Retirement

> *All of this has happened before. The old log stays in the record; the lesson
> jumps forward.*

Knowledge retires, it does not disappear. When a fact stops holding you
supersede it, and the supersede recipe is exact - follow it rather than
overwriting the old truth into silence:

1. Write the replacement as its own new engram with `status: current`. Do not
   edit the old fact in place; that skips the trail and leaves the outdated
   value searchable as current.
2. Edit the old engram: `find_replace` its frontmatter `status: current` to
   `status: superseded`, and add a `- superseded_by [[New Engram]]` relation.
3. When you know the date the old fact stopped holding, close its window in the
   same edit by adding a `valid_to: <date>` line - the real transition date,
   never a sentinel. A closed window is what lets a later search answer "what
   applied last June" with the engram that was true then.
4. Carry the lesson forward. Any insight that outlives the retired fact - why it
   failed, what to watch for - becomes a `[lesson]` bullet on the new engram.
   The experience stays time-scoped; what it taught travels on unbounded.

Say it plainly and the agent runs the whole cycle:

```text
We switched the coolant from glycol mix B to mix C on 2026-06-01. Supersede the
old engram rather than overwriting it, and carry forward why mix B ran hot.
```

By hand, step 2 is a single edit:

```sh
crystalline edit coolant-glycol-mix-b ship-ops find_replace \
  --find-text "status: current" --content "status: superseded"
```

The status words each mean one thing: `deprecated` says do not do this again,
`superseded` says a newer engram replaced this one and `archived` says retired
but kept for the record. `delete` is for mistakes, not for history - the old
log is how the ship learns it has seen this before.

## Mission 07 - Joint operations

> *Help us keep this knowledge. Transmit the plans. You are our only hope.*

A team domain is an ordinary domain whose files also live in a GitHub
repository: your local markdown stays the truth on this machine, and an origin
records which repository it tracks. Wire up the alliance once:

```sh
crystalline config set github.enabled true
crystalline connect github
crystalline domain add fleet-ops --origin alliance/fleet-ops --branch main
```

`connect github` hands you a short code to confirm in the browser - no git, no
SSH keys, just this machine's identity. From there the loop is four moves:

```sh
crystalline origin status                                   # at session start
crystalline origin update                                   # before deep work
crystalline origin share fleet-ops --title "Docking clamp timing"
crystalline origin resolve fleet-ops fleet-ops/clamp.md --keep mine
```

`share` does not merge anything - it opens a proposal and hands back a review
URL. A person at command reads it and merges on GitHub; the agent never merges
its own work, it only relays the link. If a proposal is declined, that is
normal traffic, not a failure - abandon it with `crystalline origin discard
fleet-ops --proposal 4` and move on. Some gave everything to bring the knowledge
in that repository back; a proposal is how you add to it without trampling what
it cost.

## Appendix - The ship's computer

> *This is Mother. I can tell you what I know. You only have to ask.*

The computer answers when queried and stays silent when not, so query it. Skip
quarantine - skip `verify` before you share, skip `doctor` when something feels
off - and you are the crew that ignored the protocol and let the thing aboard.
Knowledge hoarded is knowledge lost; the point of every mission above is that
what you learn reaches the next shift and the rest of the crew.

### Quick reference

| To do this | Say to your agent | Or run |
|---|---|---|
| Wire up a harness | (once, in the terminal) | `crystalline install claude-code` |
| Commission a domain | (once, in the terminal) | `crystalline domain init <path> --name <n>` then `crystalline domain add <n> <path>` |
| Capture a fact | "Capture this in ship-ops as a [gotcha]: ..." | `crystalline write <domain> "<title>" --content "..." --tags a,b --type guide` |
| Recall, scoped | "What do we know about docking?" | `crystalline search "docking" --domain ship-ops` |
| Recall, everywhere | "Search everything about coolant." | `crystalline search "coolant" --status current` |
| Read one engram | "Open the clamp gotcha." | `crystalline read <identifier> --domain <domain>` |
| Walk the graph | "Show me what connects to the coolant loop." | `crystalline context "crystalline://<domain>/<permalink>" --depth 2` |
| Refine in place | "Update the coolant engram, do not fork it." | `crystalline edit <identifier> <domain> <operation>` |
| Catch up | "What changed this week?" | `crystalline recent --timeframe 7d` |
| Tidy vocabulary | "Any tag drift in ship-ops?" | `crystalline vocabulary --domain <domain>`, `crystalline tags merge <old> <into>` |
| Share with the team | "Share the docking work as a proposal." | `crystalline origin share <domain> --title "..."` |

### Reference blocks

Recommended `status` values (guidance, not enforced): `current`, `implemented`,
`draft`, `proposed`, `idea`, `poc`, `deprecated`, `superseded`, `archived`,
`legacy`.

Observation categories: `- [decision]`, `- [fact]`, `- [pattern]`, `-
[gotcha]`, `- [convention]`, `- [lesson]`, `- [risk]`, `- [insight]`, `-
[idea]`, `- [proposal]` and `- [poc]`. Free text, so reach for the most precise
one.

Relation syntax: `- rel_type [[Other Engram]]`, or quote a multi-word type,
`- "relates to" [[Other Engram]]`.

Temporal rules: no `valid_from` means always valid, no `valid_to` means valid
forever. Set a bound only when validity is genuinely limited, as a plain ISO
date (`YYYY-MM-DD`). Never write a sentinel far-future date to mean forever -
absence already means it.

Address scheme: `crystalline://<domain>/<permalink>` is the one absolute form.
Any identifier without the scheme is domain-relative, so pass a bare permalink
and name the domain separately.

Maintenance one-liners, run them before you need them:

```sh
crystalline verify              # static check: frontmatter, links, schema
crystalline doctor              # diagnose index and service; add --fix to repair
crystalline reindex --full      # rebuild the derived index from the files
```

The index is disposable and the files are the truth, so `reindex --full` is
never a data-loss event - it is the clean-room reset that syncs the index back
to the files, ratio restored.

---

That is the whole flight. Eight missions, one ship, an agent that now reads the
log before it touches the clamps. Keep capturing, keep reconciling, retire what
stops holding and share the rest. See you around, space cowboy.
