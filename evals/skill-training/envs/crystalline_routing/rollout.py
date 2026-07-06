"""Routing benchmark rollout: the shared sandboxed Claude Code runner
from envs/common.py with the routing scorer (transcript-only; the
sandbox post-state is irrelevant for read-only routing tasks)."""
from __future__ import annotations

from pathlib import Path

from envs import common
from envs.common import TransientRolloutError  # noqa: F401  (re-export)
from envs.crystalline_routing.scoring import score_item


def _score(
    item: dict,
    sandbox: Path,
    tool_calls: list[dict],
    answer: str,
    prepared: object,
) -> tuple[int, float, list[str]]:
    del sandbox, prepared
    return score_item(item.get("expect", {}) or {}, tool_calls, answer)


def run_batch(
    *,
    items: list[dict],
    skill_content: str,
    out_root: str,
    claude_bin: str,
    claude_model: str,
    crystalline_bin: str,
    fixture_root: str,
    workers: int = 4,
    max_turns: int = 12,
    exec_timeout: int = 240,
) -> list[dict]:
    return common.run_batch(
        items=items,
        skill_content=skill_content,
        out_root=out_root,
        claude_bin=claude_bin,
        claude_model=claude_model,
        crystalline_bin=crystalline_bin,
        fixture_root=fixture_root,
        workers=workers,
        max_turns=max_turns,
        exec_timeout=exec_timeout,
        score=_score,
        default_task_type="routing",
        sandbox_prefix="cst-routing-",
    )
