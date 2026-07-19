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
includes an AI agent that starts every shift a stranger and forgets the last one,
unless you teach it to remember. Eight missions below, each runnable start to
finish: work through them and the ship keeps its own log, the agent reads it
before acting and neither of you re-derives the docking procedure from scratch
again.

Every mission is a conversation. You speak in plain human terms; the agent infers
the rest - which domain a fact belongs in, how to file it, what to search first -
then proposes and does it. The italic line after each prompt shows that inference
at work. Only the first step and a small terminal corner touch a command line. The
[reference README](../README.md) is the map; this is the flight training.

## Mission 00 - Preflight

> *Tank, load me the operations manual. A pause. I know the ship.*

Install the binary first - pick your platform from the
[install matrix](../README.md#install-the-binary) and come back. Then one command
jacks the agent in:

```sh
crystalline install claude-code
```

That single step wires the whole rig - MCP registration, the `SessionStart`
routing hook, the `Stop` capture nudge and the four skills. It is the "I know
kung fu" moment: next session the agent wakes already knowing how to route,
capture and share. The same command takes `codex` or `copilot`. This is the last
time you need a terminal. Claude Desktop skips even this: install the `.mcpb`
extension from the latest release, upload the `crystalline-memory` skill zip and
never open a terminal.

Start a session and ask what it already knows:

```text
What do you already know about this ship?
```

*A fresh install can see no domains yet - that is Mission 01. A healthy archive
keeps its sync ratio: what the agent sees and what is on disk track one to one.*

## Mission 01 - Commission the archive

> *Computer: commission the ship's archive. Log our five-year mission.*

A domain is a folder of knowledge with a `MANIFEST.md` at its root. Just tell the
agent what you want it to remember:

```text
Set up a place to keep everything about the ship - the docking gear, the
coolant loop, the vent drivers.
```

*The agent proposes a file domain named `ship-ops` in its domains folder, scaffolds
its `MANIFEST.md` and registers it, confirming the name and location first.*

Now the manifest matters. Ask the agent to fill in that `MANIFEST.md`, or edit it
yourself - its `## Scope` and `## When to Use` sections are the routing beacon the
agent reads each session to decide whether a task belongs here. Write them like
standing orders - "when asked about docking, coolant or the vent drivers, look
here". Vague scope is a computer that answers every hail with "insufficient data".
Next session the domain shows up in the agent's routing block, and you have given
it somewhere to think.

## Mission 02 - First entries

> *Captain's log, supplemental: the docking clamps, again.*

Capture is a byproduct of the work, not a chore. Say what you learned the way you
would tell a crewmate:

```text
Remember this: docking clamp 3 reads locked about half a second before it
actually seats, so wait for the green tone before cutting thrust.
```

*You named no domain and no category. The agent searches the archive first, finds
nothing, then proposes filing it in `ship-ops` as a `[gotcha]` tagged `docking`
and writes only once you say yes - it named the domain for you, so nothing lands
wrong.*

Knowledge that leans on other knowledge gets a relation. You still just talk:

```text
Also note that we decided to run the coolant loop on glycol mix B - it rides on
the vent driver firmware.
```

*The agent files a `[decision]` and, seeing the firmware engram already exists,
links them on disk as `- depends_on [[Vent Driver Firmware]]`.*

On disk each capture is a bullet like `- [gotcha] Clamp 3 seats late #docking`.
Pick a precise category (the appendix lists them); `type` and `status` have
recommended values but are guidance, not a fixed enum. Your stardate
(`recorded_at`) is filled in for you.

## Mission 03 - Recall

> *Time is a big ball of what-was-true-then. Do be careful. Spoilers.*

Ask the way it forms in your head:

```text
What do we know about the docking clamps?
```

*A narrow question, so the agent scopes itself to `ship-ops`.*

```text
What do we know about coolant - check everything.
```

*The archive is vast and every domain answers; the agent sweeps them all.* For
the neighbourhood around one engram, ask "what connects to the coolant loop?" and
the agent walks the relations from Mission 02.

Two questions, two searches. Ask *what is true now?* and the agent filters on
status; ask *what applied last June?* and it reasons over validity windows. Heed
the timey-wimey warning: an engram with no `valid_from` has always been valid and
one with no `valid_to` is valid forever, so a strict date bound sees only the
windows that exist and can skip the unbounded rows - the very ones the question
means to keep. Absence means always, so the agent leans on status for
now-versus-then, never filtering the open-ended rows into silence.

Back from leave? Ask "what changed while I was away?" and the agent pulls the
recent captures across every domain. An engram is what persists of a session after
the session is gone; write enough of them and the ship remembers what the crew
forgets.

## Mission 04 - Bulk intake

> *Legacy knowledge, like life, finds a way. Preserve it in amber, not a tar pit.*

Crystalline ships no scraper, by design: the agent is the ingester, reading a
source and distilling it into engrams, one truth per domain. The cardinal sin is
mirroring - copying a source wholesale so it looks complete, filling the gaps with
frog DNA that breeds surprises nobody signed off on. Distill the durable facts and
drop the rest. Four intake jobs, four things you say. A webpage:

```text
Read this page and remember the parts that will still be true next month - and
keep a link back to where you found it.
```

*The agent distills the durable facts into `ship-ops`, records the source URL and
date in frontmatter (`resource`, `source_date`) and drops whatever goes stale.
Webpage intake needs a harness with web access.*

Local documents on disk:

```text
Here are three spec PDFs in ./specs - pull out what is worth keeping, one topic
each.
```

*The agent proposes an engram per keeper and waits for your yes.*

A git repository:

```text
Look through this repo and remember how it is built and the conventions it
follows.
```

*The agent proposes a fresh domain for it - say `vessel-arch` - lists the engrams
it will write and holds until you approve. Distilled architecture, not a copy of
the code.*

Your team wiki:

```text
Our team wiki has years of pages - bring over the ones still worth keeping and
leave the rest.
```

*It distills the keepers into `ship-ops` and leaves the fossils in the amber.*

A legacy markdown tree that is already frontmatter-shaped is the one exception
that stays in the terminal: an import command in the
[terminal corner](#the-terminal-corner) folds it into the domain, source tree
untouched. They spared no expense on the old archive, and it shows: half is
scaffold. The Jurassic Park lesson holds - the crew got so preoccupied with
whether they could ingest all of it that nobody stopped to ask whether they
should. Distill.

## Mission 05 - Reconciliation

> *If my calculations are right, when two engrams disagree you are about to see
> some serious reconciliation.*

Two standing orders keep the timeline single: search before you write, and edit
over create. When new knowledge lands on a topic that already has an owner, that
owner gets refined in place - no rival engram that contradicts it. Nudge the agent
when it forgets:

```text
Do we already have something on coolant? Update it instead of starting a new one.
```

*The agent searches `ship-ops` first and, when an owner exists, refines it in
place rather than forking a duplicate.*

Reconcile in place, never as an append log. An engram is what is true now, so the
changed fact gets replaced where it stands; nobody staples a dated `## Update`
section under it. "Checked the source, nothing changed" is one frontmatter field,
`last_verified: <date>`, that the agent keeps current. A stale engram nobody has
re-checked is the photograph where the crew is slowly fading out; a current
`last_verified` keeps everyone in the picture.

Vocabulary drifts the same way a timeline does - `docking` here, `docking-clamp`
there, the same idea under two names. Just ask:

```text
Have our tags drifted - anything meaning the same thing under two names?
```

*The agent surveys the vocabulary and flags near-duplicate clusters like `docking`
and `docking-clamp`.* Folding them is a deliberate bulk rewrite, so it stays in
the terminal: a merge command in the [terminal corner](#the-terminal-corner)
rewrites the tag everywhere and records `- docking-clamp -> docking` in the
MANIFEST's `## Tag Aliases` section, so the old name still finds everything. The
agent proposes the alias when it spots the drift; the merge is yours to run.

## Mission 06 - Retirement

> *All of this has happened before. The old log stays in the record; the lesson
> jumps forward.*

Knowledge retires, it does not disappear. When a fact stops holding it is
superseded, not overwritten into silence, and on disk the recipe is exact:

1. The replacement is written as a new engram with `status: current` - not edited
   into the old fact, which would leave the outdated value searchable as current.
2. The old engram is edited: a `find_replace` flips its frontmatter
   `status: current` to `status: superseded`, and a `- superseded_by [[New
   Engram]]` relation is added.
3. If the date it stopped holding is known, the same edit adds a `valid_to:
   <date>` line - the real transition date, never a sentinel - so a later search
   can answer "what applied last June" with the engram that was true then.
4. The lesson carries forward: any insight that outlives the retired fact becomes
   a `[lesson]` bullet on the new engram. The experience stays time-scoped; what
   it taught travels on unbounded.

You do not run those steps. You just say what happened:

```text
The coolant mix changed on the first of June - we are off glycol mix B and
running mix C now. Mix B ran hot; make sure we remember why.
```

*The agent runs the recipe for you: mix C becomes a new `current` engram, the old
one flips to `superseded` with a `- superseded_by` link, its window closes at
2026-06-01 and the "why B ran hot" lesson moves to a `[lesson]` bullet.*

The status words: `deprecated` says do not do this again, `superseded` says a
newer engram replaced this one and `archived` says retired but kept for the
record. `delete` is for mistakes, not history - the old log is how the ship learns
it has seen this before.

## Mission 07 - Joint operations

> *Help us keep this knowledge. Transmit the plans. You are our only hope.*

A team domain is an ordinary domain whose files also live in a GitHub repository:
your local markdown stays the truth, and an origin records which repository it
tracks. Wiring up the alliance is a conversation, not a config file:

```text
Let us start sharing knowledge with the rest of the fleet.
```

*The agent turns on GitHub team sharing and connects this machine, handing you a
short browser code to confirm - no git, no SSH keys, just this machine's identity.*

```text
Pull in the fleet's shared knowledge repo, alliance/fleet-ops.
```

*The agent registers it as a team domain - `fleet-ops` - tracking that
repository's main branch and downloads it.*

From there the loop is a rhythm you speak: ask where the domain stands at session
start, pull the team's merged work before you dig in and share when your own work
is worth it:

```text
My docking clamp notes are worth sharing - send them to the fleet for review.
```

*The agent opens a proposal from your local changes and hands back a review URL.*
A person at command reads it and merges on GitHub; the agent never merges its own
work, only relays the link. A declined proposal is normal - refine and share
again, or let it lapse (dropping one for good is a terminal-corner move:
`crystalline origin discard`). If two edits collide, ask the agent to resolve
it, keeping your side or theirs. Some gave everything to bring that knowledge back; a proposal is how
you add to it without trampling what it cost.

## Appendix - The ship's computer

> *This is Mother. I can tell you what I know. You only have to ask.*

The computer answers when queried and stays silent when not, so query it.
Knowledge hoarded is knowledge lost - every mission above is about what you learn
reaching the next shift and the rest of the crew. Below is a quick reference; the
terminal corner is optional.

### Quick reference

| To do this | Say to your agent |
|---|---|
| Commission a domain | "Give me somewhere to remember everything about the ship." |
| Capture a fact | "Remember this: the port clamp sticks in the cold." |
| Recall, scoped | "What do we know about docking?" |
| Recall, everywhere | "What do we know about coolant - check everything." |
| Read one engram | "Show me the clamp gotcha." |
| Walk the graph | "What connects to the coolant loop?" |
| Refine in place | "Update what we have on coolant, do not start a new note." |
| Catch up | "What changed while I was away?" |
| Recall what was true then | "What applied last June?" |
| Tidy vocabulary | "Have our tags drifted?" |
| Retire a fact | "The old coolant mix is retired - update the archive." |
| Share with the team | "These notes are worth sharing - send them for review." |

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
forever. Set a bound only when validity is genuinely limited, as a plain ISO date
(`YYYY-MM-DD`). Never write a sentinel far-future date to mean forever - absence
already means it.

Address scheme: `crystalline://<domain>/<permalink>` is the one absolute form. Any
identifier without the scheme is domain-relative, so pass a bare permalink and
name the domain separately.

### The terminal corner

Optional power-user territory - a reader who never opens a terminal can skip it.
But skip the quarantine checks before you share and you are the crew that let the
thing aboard, so `verify` and `doctor` earn their keep. These are the only
commands the missions did not hand to the agent:

```sh
crystalline install claude-code                     # wire up a harness (Mission 00)
crystalline import ./old-notes --domain ship-ops    # convert a legacy markdown tree
crystalline tags rename <old> <new>                 # rename a tag everywhere
crystalline tags merge <old> <into>                 # fold one tag into another
crystalline origin discard <domain> --proposal <n>  # drop a declined proposal for good
crystalline verify                                  # static check: frontmatter, links, schema
crystalline doctor                                  # diagnose index and service; add --fix to repair
crystalline reindex --full                          # rebuild the derived index from the files
```

The index is disposable and the files are the truth, so `reindex --full` is never
a data-loss event - it is the clean-room reset that syncs the index back to the
files, ratio restored.

---

That is the whole flight. Eight missions, one ship, an agent that now reads the
log before it touches the clamps. Keep capturing, keep reconciling, retire what
stops holding and share the rest. See you around, space cowboy.
