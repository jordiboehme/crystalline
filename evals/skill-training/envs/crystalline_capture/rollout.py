"""Capture benchmark rollout: the shared sandboxed Claude Code runner
with a pre-run state snapshot (verify report + file listing) and the
capture scorer, which reads the sandbox post-state before cleanup."""
from __future__ import annotations

from pathlib import Path

from envs import common
from envs.common import TransientRolloutError  # noqa: F401  (re-export)
from envs.crystalline_capture.scoring import score_item, snapshot


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
    max_turns: int = 16,
    exec_timeout: int = 300,
) -> list[dict]:
    def _prepare(item: dict, sandbox: Path) -> dict:
        del item
        return snapshot(sandbox, crystalline_bin)

    def _score(
        item: dict,
        sandbox: Path,
        tool_calls: list[dict],
        answer: str,
        prepared: dict,
    ) -> tuple[int, float, list[str]]:
        return score_item(
            item.get("expect", {}) or {},
            tool_calls,
            answer,
            sandbox,
            prepared,
            crystalline_bin,
        )

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
        prepare=_prepare,
        default_task_type="capture",
        sandbox_prefix="cst-capture-",
    )
