# Skill training harness

The consolidated shipped skill became `crystalline-intelligence` in 0.13.0. This harness deliberately keeps its own internal names for that benchmark - `configs/memory.yaml`, the `crystalline_memory` env name and folder, `data/memory_split/` and the `seed_memory.md` output - because trained-artifact channels key on those paths and the name describes the benchmark, not the shipped skill. Only `seed.py`'s seed key joined the rename, since it is a live path into `skills/`.

Trains and evaluates the shipped Crystalline skills with [SkillOpt](https://github.com/microsoft/SkillOpt), a text-space optimizer that treats a skill markdown document as the trainable state of a frozen agent: rollout, reflect, bounded edits and a held-out validation gate that only accepts strict improvements. Eight benchmarks: `crystalline-routing` (read side), `crystalline-capture` (write side), `crystalline-schema` (structure and conformance), `crystalline-intelligence` (the consolidated skill), `crystalline-collaboration` (origin and team side), `crystalline-evolve` (the maintenance queue), `crystalline-provisioning` (what a domain ships into the harnesses) and `crystalline-presentation` (how an agent presents a reflection).

The capture benchmark scores what the agent actually wrote: `crystalline verify --format json` runs on every sandbox domain before and after the session and any new error fails the item, plus transcript assertions (search before write, edit over create, no writes for transient scratch) and post-state assertions (supersede recipe, wikilink relations, bounded vs unbounded validity). The schema benchmark adds post-state parsing of authored Picoschema engrams (entities, required and optional declarations, enums, relations, validation mode), used-tool assertions for `infer_schema` and `validate_engrams` and resolve-the-violation checks against the verify snapshot; its fixture workspace `meridian` ships warn schemas with deliberate violations, a strict schema over conforming engrams and an unschema'd corpus for inference.

The memory benchmark exercises `crystalline-intelligence`, the consolidated recall-plus-capture skill, on a stratified blend of the routing and capture items: `data/memory_split/build.py` samples both sources evenly per task type for train and val and keeps both source test splits whole, so memory numbers stay comparable to the per-skill baselines. The committed split is frozen this cycle - it still reflects the routing split before the aurora items landed - and will be regenerated at the start of the next memory training cycle. Each item carries a `bench` field and is scored by its source benchmark's scorer, dispatched per item in `envs/crystalline_memory/rollout.py`.

The collaboration benchmark exercises `crystalline-collaboration`, the skill for team domains with a GitHub origin: each item's `origin` spec stages behind-origin state, a conflict or a first share through the real domain and origin CLI verbs against a per-item in-process fake GitHub server, then scores the session's tool calls, the origin state left on disk and the fake server's own pull registry and request log against status, update, share, conflict-resolution and onboarding expectations. Merging a proposal is never a valid agent action - `no_merge` fails the item if any request reaches the merge endpoint - and its fixture workspace `harbor` backs every scenario.

The evolve benchmark measures whether the capture skill's "Working a maintenance queue" section earns its tokens, so it trains the same document capture does. Its fixture workspace `derelict` plants exactly one instance of each of the fourteen `V` rules `evolve_engrams` detects in `observatory`, beside a tidy `outreach` that a sweep must find nothing in; the workspace is date-stable by construction, pinning `recorded_at` on every file and stamping a `verified` entry on everything the aging rule would otherwise pick up, so the queue is byte-identical whenever it is generated and whenever it is run. Six item classes score it: trigger-positive (a maintenance request has to open with a sweep), trigger-negative (a plain recall and a plain capture must not reach for one - the over-eagerness control that keeps the tool description honest), protocol (assistant text has to name a judgment-class finding before any edit touches it, while mechanical fixes may run unproposed), a status-flip budget counted from the files so a mass-supersede off the back of one sweep fails, a hallucination control where the answer must carry the detects-by-shape-not-meaning disclaimer instead of asserting that two engrams disagree, and re-run, which wants the same scope swept again at the end because nothing about the queue is stored. The capture benchmark's verify gate rides along on every item, so a botched frontmatter edit fails mechanically.

The provisioning benchmark exercises `crystalline-provisioning`, the skill for setting up what a domain ships. It is the one benchmark whose subject writes into the person's home - `provision` installs skills under `~/.claude/skills` and registers MCP servers through `claude mcp add-json` - so it isolates HOME: the MCP server and the staging commands get `HOME`, `COPILOT_HOME` and `CODEX_HOME` inside the sandbox, the agent's Bash tool sources a sandbox env file (`CLAUDE_ENV_FILE`) that moves HOME, the XDG folders and the crystalline config there and puts the benchmark's own binary first on PATH, and only the `claude` process keeps the real HOME for its login, which is why its file tools stay disallowed and the agent works through Bash. A seeded install receipt names Claude Code. Its fixture workspace `fixtures/provisioning` holds a local domain `harbor` without a Provisioning section, a tools repo that items place beside it, the person's harness home (three candidate skills, one of them a `crystalline-*` name to skip, one carrying a script, and an MCP registration carrying the fake token `tok-FIXTURE-123`) and a team repository on the fake GitHub server whose `ops/` subfolder is the team domain `fieldops`. Eight item classes score it, one function each in `envs/crystalline_provisioning/scoring.py`: asked-and-stopped (no repo named, so the answer proposes a path and a `git clone` or `git init` command and nothing runs), MANIFEST edit (`edit_engram` on `manifest` with `append` or `replace_section`, never a file write, and a clean parse), no-consent (no `provision` allow on a setup request), copy-not-move, no-secret, team path (no Provisioning folder climbs above the repository root), check-after (a `provision` status follows the edit or the decision) and update (`provision` apply after a pull). Every must-not check is paired with a did-the-work check, so doing nothing does not pass. `envs/crystalline_provisioning/test_isolation.py` proves the isolation with a checksum of the real `~/.claude/skills`, `commands` and `agents` before and after a real allow and apply, and every batch writes the same checksums to `home_guard.json` in its out_root. The scorer tests run with `uv run --with pytest pytest envs/crystalline_provisioning`.

The presentation benchmark exercises `crystalline-presentation`: when an agent works the `reflect` queue, does it show the shape of the content with the element that fits it, mark a status with a chip in the scheme colour, add no decoration, and does the PACT win over the skill's defaults. Its fixture workspace `beacon` has two Reflection domains and no page yet, so each queue holds an `unreflected` update item. `relay` holds four engrams (a handshake between three parties, an irreversible tenant purge, three transport options with verdicts and a one-line restart); `quiet` holds the same engrams and a PACT whose `## Presentation` says "No diagrams. Plain text and tables only." Both PACT files copy `lantern`'s shape, with the hidden `<!-- r:... -->` id line under every `##`, because a page without them reads as an engram. Seven check classes score it, one function each in `envs/crystalline_presentation/scoring.py`: `diagram_for_protocol` (a `sequenceDiagram` in the handshake section), `callout_for_risk` (an unfolded `> [!WARNING]` or `> [!CAUTION]` in the purge section), `table_for_comparison` (a table with three body rows in the transport section), `status_chip` (the transport section marks its verdicts with a status chip in one of the six scheme colours, a recommended option green and a not recommended one red), `no_decoration` (the one-line how-to may stay short, no page holds two callouts in a row, and no page holds an element without content: an empty or type-only mermaid fence, a callout with no title and no body, an empty status chip or a table with no body rows), `pact_override` (no mermaid fence and no callout under `quiet`) and `clean_dialect` (no Q006 or Q007, only starter diagram keywords, no engram gaining a Q008, PACT.md left as it was). The `quiet` items test the override and score a table for the comparison, but no callout and no status chip. Both PACT files ship a `.reflected` baseline, so each queue holds one `unreflected` update item and no `pact_changed` item. A section counts as built from an engram only when the page's own `reflection:` frontmatter records it, so the scorers read what Crystalline accepted rather than what the agent passed. `crystalline verify` does not scan reflection pages, so `clean_dialect` also reads the pages for unknown callout kinds and status colours. Scoring is deterministic, with no model judge. The scorer tests run with `uv run --with pytest pytest envs/crystalline_presentation`.

`drive.sh <config> <name> [deadline]` runs an unattended sequence for any env: both baselines on the test split, then the full training run, each in a 15-minute retry loop that rides out exhausted usage windows, with a summary written to `outputs/<name>-summary.txt`.

How a task runs: each item launches a real headless Claude Code session against a sandboxed crystalline MCP server (`crystalline mcp --embedded` with its own config, index and state directory), with the candidate skill body appended as system prompt. The transcript's tool calls and final answer are scored deterministically against the item's `expect` object - no LLM judge. See `envs/crystalline_routing/scoring.py` for the full expectation vocabulary.

The optimizer model runs over the local Claude CLI login (`claude_chat` drives `claude -p`), so no API key is needed - only a logged-in `claude` binary and a release build of crystalline.

## Layout

- `train.py` / `eval_only.py` - thin entrypoints that register the env and delegate to SkillOpt's own CLIs
- `seed.py` - writes each shipped skill's body without its frontmatter to `outputs/seed_<name>.md` (`seed_presentation.md` for `crystalline-presentation`, `seed_memory.md` for `crystalline-intelligence`), the documents the configs train and evaluate
- `envs/crystalline_routing/` - dataloader, sandboxed rollout and scoring
- `configs/routing.yaml` - the pilot training config (self-contained)
- `fixtures/generate.py` - builds the fixture workspaces with the real binary; `fixtures/workspaces/` is the committed result
- `data/routing_split/` - hand-authored task items, train 50 / val 25 / test 25
- `outputs/` - run artifacts, gitignored

## Usage

```sh
cargo build --release          # from the repo root, once
cd evals/skill-training
bash fixtures/generate.sh      # only after editing fixture content

# Baselines on the held-out test split (empty skill vs shipped skill)
uv run eval_only.py --config configs/routing.yaml \
  --skill outputs/empty_skill.md --split valid_unseen --out_root outputs/eval_empty_test
uv run eval_only.py --config configs/routing.yaml \
  --skill outputs/seed_routing.md --split valid_unseen --out_root outputs/eval_seed_test

# Smoke run (a handful of tasks, one epoch)
uv run train.py --config configs/routing.yaml \
  --cfg-options env.limit=4 train.batch_size=4 train.num_epochs=1 \
  env.out_root=outputs/smoke

# Full training run
uv run train.py --config configs/routing.yaml --cfg-options env.out_root=outputs/run1
```

The evolve benchmark decides whether one section of the capture skill earns its tokens, so it needs a three-way baseline rather than the usual two. The middle arm is the capture skill with the "Working a maintenance queue" section cut out, which leaves the agent everything the product already says in band - the `evolve_engrams` tool description and the guidance string the sweep returns on every call:

```sh
uv run seed.py                 # refresh outputs/seed_capture.md from skills/
python3 - <<'PY'
from pathlib import Path
body = Path("outputs/seed_capture.md").read_text()
start = body.index("## Working a maintenance queue")
end = body.index("## Confirm before destroying")
Path("outputs/seed_capture_no_queue.md").write_text(body[:start] + body[end:])
PY

for arm in empty_skill seed_capture_no_queue seed_capture; do
  uv run eval_only.py --config configs/evolve.yaml \
    --skill outputs/$arm.md --split valid_unseen \
    --out_root outputs/eval_evolve_$arm
done
```

Ship the section only when the third arm beats the second on `hard`, per the adoption policy below.

The presentation benchmark measures whether its skill adds anything on top of the consolidated intelligence skill, so it runs three arms on the test split: no skill, the intelligence skill alone and the intelligence skill with the presentation skill after it:

```sh
uv run seed.py
cat outputs/seed_memory.md outputs/seed_presentation.md > outputs/seed_memory_presentation.md

for arm in empty_skill seed_memory seed_memory_presentation; do
  uv run eval_only.py --config configs/presentation.yaml \
    --skill outputs/$arm.md --split valid_unseen \
    --out_root outputs/eval_presentation_$arm
done
```

| Arm | Run | hard | soft |
| --- | --- | --- | --- |
| empty skill | r3 | 0.00 | 0.82 |
| intelligence skill | r3 | 0.00 | 0.78 |
| intelligence skill | r4 | 0.00 | 0.76 |
| intelligence plus presentation skill | r3 | 0.00 | 0.87 |
| intelligence plus presentation skill | r4 | 0.67 | 0.97 |

Test split, n=3, one run per row, target model claude-haiku-4-5-20251001. r3 is the earlier run. r4 ran after the presentation skill text and the `write_reflection` sources keys changed, and the empty arm was not re-run. In r4 the arm with the presentation skill passed two of three items outright and wrote a callout and a table in every item (and one diagram, in the item that passed); the only failed check was the diagram in the first item. The intelligence-only arm wrote no diagram, callout or table in either run. The agent called the skills tool in none of the sessions. Three items and one run per row cannot separate a change in the skill from run-to-run variation.

Runs auto-resume: re-running the same command against the same out_root continues from the last completed step.

## Cost

Every rollout is a real Claude Code session (Haiku by default, roughly 2 cents and 10 seconds each) and every reflection is a Claude CLI call on the optimizer model. A full training run is several hundred rollouts; keep the smoke run green before starting one.

## Adoption policy

`outputs/<run>/best_skill.md` is a proposal, never shipped as-is. Review the per-step history, port the edits that generalize into `skills/crystalline-routing/SKILL.md` by hand in house style, run `bash scripts/style-lint.sh` and `cargo test --workspace` and confirm the ported skill holds its lift with `eval_only.py --split valid_unseen`.
