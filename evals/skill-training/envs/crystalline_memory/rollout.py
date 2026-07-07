"""Memory benchmark rollout: the shared sandboxed Claude Code runner
with per-item scorer dispatch. Routing items score on the transcript
alone; capture items snapshot the sandbox before the session and diff
the verify layer and post-state afterwards, exactly as in their source
benchmarks."""
from __future__ import annotations

from pathlib import Path

from envs import common
from envs.common import TransientRolloutError  # noqa: F401  (re-export)
from envs.crystalline_capture import scoring as capture_scoring
from envs.crystalline_memory.dataloader import bench_of
from envs.crystalline_routing import scoring as routing_scoring


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
    def _prepare(item: dict, sandbox: Path) -> dict | None:
        if bench_of(item) == "capture":
            return common.snapshot(sandbox, crystalline_bin)
        return None

    def _score(
        item: dict,
        sandbox: Path,
        tool_calls: list[dict],
        answer: str,
        prepared: dict | None,
    ) -> tuple[int, float, list[str]]:
        expect = item.get("expect", {}) or {}
        if bench_of(item) == "routing":
            return routing_scoring.score_item(expect, tool_calls, answer)
        return capture_scoring.score_item(
            expect, tool_calls, answer, sandbox, prepared, crystalline_bin
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
        default_task_type="memory",
        sandbox_prefix="cst-memory-",
    )
