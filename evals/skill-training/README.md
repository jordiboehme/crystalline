# Skill training harness

Trains and evaluates the shipped Crystalline skills with [SkillOpt](https://github.com/microsoft/SkillOpt), a text-space optimizer that treats a skill markdown document as the trainable state of a frozen agent: rollout, reflect, bounded edits and a held-out validation gate that only accepts strict improvements. The pilot benchmark covers `crystalline-routing`.

How a task runs: each item launches a real headless Claude Code session against a sandboxed crystalline MCP server (`crystalline mcp --embedded` with its own config, index and state directory), with the candidate skill body appended as system prompt. The transcript's tool calls and final answer are scored deterministically against the item's `expect` object - no LLM judge. See `envs/crystalline_routing/scoring.py` for the full expectation vocabulary.

The optimizer model runs over the local Claude CLI login (`claude_chat` drives `claude -p`), so no API key is needed - only a logged-in `claude` binary and a release build of crystalline.

## Layout

- `train.py` / `eval_only.py` - thin entrypoints that register the env and delegate to SkillOpt's own CLIs
- `envs/crystalline_routing/` - dataloader, sandboxed rollout and scoring
- `configs/routing.yaml` - the pilot training config (self-contained)
- `fixtures/generate.py` - builds the fixture workspaces with the real binary; `fixtures/workspaces/` is the committed result
- `data/routing_split/` - hand-authored task items, train 40 / val 20 / test 20
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

Runs auto-resume: re-running the same command against the same out_root continues from the last completed step.

## Cost

Every rollout is a real Claude Code session (Haiku by default, roughly 2 cents and 10 seconds each) and every reflection is a Claude CLI call on the optimizer model. A full training run is several hundred rollouts; keep the smoke run green before starting one.

## Adoption policy

`outputs/<run>/best_skill.md` is a proposal, never shipped as-is. Review the per-step history, port the edits that generalize into `skills/crystalline-routing/SKILL.md` by hand in house style, run `bash scripts/style-lint.sh` and `cargo test --workspace` and confirm the ported skill holds its lift with `eval_only.py --split valid_unseen`.
