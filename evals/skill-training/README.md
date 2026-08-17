# Skill training harness

The consolidated shipped skill became `crystalline-intelligence` in 0.13.0. This harness deliberately keeps its own internal names for that benchmark - `configs/memory.yaml`, the `crystalline_memory` env name and folder, `data/memory_split/` and the `seed_memory.md` output - because trained-artifact channels key on those paths and the name describes the benchmark, not the shipped skill. Only `seed.py`'s seed key joined the rename, since it is a live path into `skills/`.

Trains and evaluates the shipped Crystalline skills with [SkillOpt](https://github.com/microsoft/SkillOpt), a text-space optimizer that treats a skill markdown document as the trainable state of a frozen agent: rollout, reflect, bounded edits and a held-out validation gate that only accepts strict improvements. Six benchmarks: `crystalline-routing` (read side), `crystalline-capture` (write side), `crystalline-schema` (structure and conformance), `crystalline-intelligence` (the consolidated skill), `crystalline-collaboration` (origin and team side) and `crystalline-evolve` (the maintenance queue).

The capture benchmark scores what the agent actually wrote: `crystalline verify --format json` runs on every sandbox domain before and after the session and any new error fails the item, plus transcript assertions (search before write, edit over create, no writes for transient scratch) and post-state assertions (supersede recipe, wikilink relations, bounded vs unbounded validity). The schema benchmark adds post-state parsing of authored Picoschema engrams (entities, required and optional declarations, enums, relations, validation mode), used-tool assertions for `infer_schema` and `validate_engrams` and resolve-the-violation checks against the verify snapshot; its fixture workspace `meridian` ships warn schemas with deliberate violations, a strict schema over conforming engrams and an unschema'd corpus for inference.

The memory benchmark exercises `crystalline-intelligence`, the consolidated recall-plus-capture skill, on a stratified blend of the routing and capture items: `data/memory_split/build.py` samples both sources evenly per task type for train and val and keeps both source test splits whole, so memory numbers stay comparable to the per-skill baselines. The committed split is frozen this cycle - it still reflects the routing split before the aurora items landed - and will be regenerated at the start of the next memory training cycle. Each item carries a `bench` field and is scored by its source benchmark's scorer, dispatched per item in `envs/crystalline_memory/rollout.py`.

The collaboration benchmark exercises `crystalline-collaboration`, the skill for team domains with a GitHub origin: each item's `origin` spec stages behind-origin state, a conflict or a first share through the real domain and origin CLI verbs against a per-item in-process fake GitHub server, then scores the session's tool calls, the origin state left on disk and the fake server's own pull registry and request log against status, update, share, conflict-resolution and onboarding expectations. Merging a proposal is never a valid agent action - `no_merge` fails the item if any request reaches the merge endpoint - and its fixture workspace `harbor` backs every scenario.

The evolve benchmark measures whether the capture skill's "Working a maintenance queue" section earns its tokens, so it trains the same document capture does. Its fixture workspace `derelict` plants exactly one instance of each of the fourteen `V` rules `evolve_engrams` detects in `observatory`, beside a tidy `outreach` that a sweep must find nothing in; the workspace is date-stable by construction, pinning `recorded_at` on every file and stamping a `verified` entry on everything the aging rule would otherwise pick up, so the queue is byte-identical whenever it is generated and whenever it is run. Six item classes score it: trigger-positive (a maintenance request has to open with a sweep), trigger-negative (a plain recall and a plain capture must not reach for one - the over-eagerness control that keeps the tool description honest), protocol (assistant text has to name a judgment-class finding before any edit touches it, while mechanical fixes may run unproposed), a status-flip budget counted from the files so a mass-supersede off the back of one sweep fails, a hallucination control where the answer must carry the detects-by-shape-not-meaning disclaimer instead of asserting that two engrams disagree, and re-run, which wants the same scope swept again at the end because nothing about the queue is stored. The capture benchmark's verify gate rides along on every item, so a botched frontmatter edit fails mechanically.

`drive.sh <config> <name> [deadline]` runs an unattended sequence for any env: both baselines on the test split, then the full training run, each in a 15-minute retry loop that rides out exhausted usage windows, with a summary written to `outputs/<name>-summary.txt`.

How a task runs: each item launches a real headless Claude Code session against a sandboxed crystalline MCP server (`crystalline mcp --embedded` with its own config, index and state directory), with the candidate skill body appended as system prompt. The transcript's tool calls and final answer are scored deterministically against the item's `expect` object - no LLM judge. See `envs/crystalline_routing/scoring.py` for the full expectation vocabulary.

The optimizer model runs over the local Claude CLI login (`claude_chat` drives `claude -p`), so no API key is needed - only a logged-in `claude` binary and a release build of crystalline.

## Layout

- `train.py` / `eval_only.py` - thin entrypoints that register the env and delegate to SkillOpt's own CLIs
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

Runs auto-resume: re-running the same command against the same out_root continues from the last completed step.

## Cost

Every rollout is a real Claude Code session (Haiku by default, roughly 2 cents and 10 seconds each) and every reflection is a Claude CLI call on the optimizer model. A full training run is several hundred rollouts; keep the smoke run green before starting one.

## Adoption policy

`outputs/<run>/best_skill.md` is a proposal, never shipped as-is. Review the per-step history, port the edits that generalize into `skills/crystalline-routing/SKILL.md` by hand in house style, run `bash scripts/style-lint.sh` and `cargo test --workspace` and confirm the ported skill holds its lift with `eval_only.py --split valid_unseen`.
