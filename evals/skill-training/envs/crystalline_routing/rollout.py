"""Sandboxed Claude Code rollout for crystalline routing tasks.

Each task runs a real headless Claude Code session against an isolated
crystalline MCP server:

1. Copy the item's fixture workspace domains into a temp sandbox.
2. Register each domain into a sandbox-private config and index
   (XDG_STATE_HOME and XDG_CONFIG_HOME point into the sandbox, so the
   single-instance lock and the state never touch the user's own
   crystalline installation).
3. Launch ``claude -p`` with the candidate skill as an appended system
   prompt, the sandbox MCP server as the only MCP config and the file
   tools disallowed so the agent cannot bypass the knowledge tools by
   grepping the fixture files.
4. Parse the stream-json transcript into tool calls plus the final
   answer, then score both deterministically (see scoring.py).

Results are resume-aware per out_root: a task whose result.json already
exists is loaded, not re-run, matching the built-in envs' behavior.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from envs.crystalline_routing.scoring import score_item

DISALLOWED_TOOLS = [
    "Bash", "Read", "Grep", "Glob", "Edit", "Write",
    "WebSearch", "WebFetch", "Task", "NotebookEdit", "TodoWrite",
]

TRANSIENT_API_STATUS = {401, 403, 429, 500, 502, 503, 529}


class TransientRolloutError(RuntimeError):
    """The session failed for infrastructure reasons (rate limit, auth,
    API outage), not agent behavior. Such a task must never be scored or
    cached: raising aborts the batch loudly and a later re-run of the
    same command resumes past the tasks that did complete."""


def _detect_transient(events: list[dict]) -> str:
    for event in events:
        if event.get("type") == "rate_limit_event":
            info = event.get("rate_limit_info", {}) or {}
            if info.get("status") == "rejected":
                return f"rate limited ({info.get('rateLimitType', 'unknown')})"
        if event.get("type") == "result":
            status = event.get("api_error_status")
            if status in TRANSIENT_API_STATUS:
                return f"API error {status}: {str(event.get('result'))[:200]}"
            text = str(event.get("result") or "")
            if "hit your session limit" in text or "usage limit" in text.lower():
                return f"usage limit: {text[:200]}"
    return ""


def _run(cmd: list[str], env: dict, timeout: int) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd, capture_output=True, text=True, encoding="utf-8",
        errors="replace", timeout=timeout, env=env,
    )


def _setup_sandbox(
    sandbox: Path,
    fixture_dir: Path,
    crystalline_bin: str,
) -> Path:
    """Copy fixture domains in and register them; return the mcp config path."""
    domains_src = fixture_dir / "domains"
    domains_dst = sandbox / "domains"
    shutil.copytree(domains_src, domains_dst)
    (sandbox / "state").mkdir()
    (sandbox / "xdg-config").mkdir()
    (sandbox / "work").mkdir()

    config = sandbox / "config.yaml"
    db = sandbox / "index.db"
    env = dict(
        os.environ,
        XDG_STATE_HOME=str(sandbox / "state"),
        XDG_CONFIG_HOME=str(sandbox / "xdg-config"),
    )
    for domain_dir in sorted(domains_dst.iterdir()):
        if not domain_dir.is_dir():
            continue
        proc = _run(
            [
                crystalline_bin, "--db", str(db),
                "domain", "add", domain_dir.name, str(domain_dir),
                "--config", str(config),
            ],
            env=env, timeout=60,
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"domain add {domain_dir.name} failed: {proc.stderr.strip()}"
            )

    mcp_config = sandbox / "mcp.json"
    mcp_config.write_text(json.dumps({
        "mcpServers": {
            "crystalline": {
                "command": crystalline_bin,
                "args": [
                    "mcp", "--embedded",
                    "--db", str(db),
                    "--config", str(config),
                ],
                "env": {
                    "XDG_STATE_HOME": str(sandbox / "state"),
                    "XDG_CONFIG_HOME": str(sandbox / "xdg-config"),
                },
            }
        }
    }))
    return mcp_config


def _parse_transcript(raw: str) -> tuple[list[dict], str, dict, list[dict]]:
    """Extract (tool_calls, final_answer, stats, events) from stream-json output."""
    tool_calls: list[dict] = []
    answer = ""
    stats: dict = {}
    events: list[dict] = []
    for line in raw.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        events.append(event)
        if event.get("type") == "assistant":
            for block in event.get("message", {}).get("content", []) or []:
                if block.get("type") == "tool_use":
                    tool_calls.append({
                        "name": block.get("name", ""),
                        "input": block.get("input", {}) or {},
                    })
        elif event.get("type") == "result":
            answer = str(event.get("result") or "")
            stats = {
                "subtype": event.get("subtype"),
                "num_turns": event.get("num_turns"),
                "duration_ms": event.get("duration_ms"),
                "total_cost_usd": event.get("total_cost_usd"),
            }
    return tool_calls, answer, stats, events


def rollout_one(
    item: dict,
    skill_content: str,
    task_dir: Path,
    *,
    claude_bin: str,
    claude_model: str,
    crystalline_bin: str,
    fixture_root: Path,
    max_turns: int,
    exec_timeout: int,
) -> dict:
    result_path = task_dir / "result.json"
    if result_path.exists():
        with open(result_path, encoding="utf-8") as f:
            return json.load(f)

    task_dir.mkdir(parents=True, exist_ok=True)
    fixture_dir = fixture_root / str(item["workspace"])
    if not fixture_dir.is_dir():
        raise FileNotFoundError(f"fixture workspace missing: {fixture_dir}")

    sandbox = Path(tempfile.mkdtemp(prefix="cst-routing-"))
    fail_reason = ""
    tool_calls: list[dict] = []
    answer = ""
    stats: dict = {}
    try:
        mcp_config = _setup_sandbox(sandbox, fixture_dir, crystalline_bin)

        skill_path = sandbox / "skill.md"
        skill_path.write_text(skill_content or "", encoding="utf-8")

        cmd = [
            claude_bin, "-p", str(item["question"]),
            "--model", claude_model,
            "--mcp-config", str(mcp_config),
            "--strict-mcp-config",
            "--setting-sources", "",
            "--disallowedTools", *DISALLOWED_TOOLS,
            "--allowedTools", "mcp__crystalline",
            "--output-format", "stream-json", "--verbose",
            "--max-turns", str(max_turns),
        ]
        if skill_content.strip():
            cmd.extend(["--append-system-prompt-file", str(skill_path)])

        try:
            proc = subprocess.run(
                cmd, capture_output=True, text=True, encoding="utf-8",
                errors="replace", timeout=exec_timeout,
                cwd=str(sandbox / "work"), stdin=subprocess.DEVNULL,
            )
            raw = proc.stdout or ""
            if proc.returncode != 0:
                fail_reason = f"claude exited {proc.returncode}: {(proc.stderr or '')[:500]}"
        except subprocess.TimeoutExpired as exc:
            raw = (exc.stdout or b"")
            if isinstance(raw, bytes):
                raw = raw.decode("utf-8", errors="replace")
            fail_reason = f"claude timed out after {exec_timeout}s"

        (task_dir / "transcript.jsonl").write_text(raw, encoding="utf-8")
        tool_calls, answer, stats, events = _parse_transcript(raw)

        transient = _detect_transient(events)
        if not transient and fail_reason and not any(
            e.get("type") == "result" for e in events
        ):
            # A nonzero exit without any result event is an infra
            # failure too (crashed CLI, killed process), not behavior.
            transient = fail_reason
        if transient:
            raise TransientRolloutError(
                f"task {item['id']}: {transient}"
            )
    finally:
        shutil.rmtree(sandbox, ignore_errors=True)

    hard, soft, failed_checks = score_item(
        item.get("expect", {}) or {}, tool_calls, answer,
    )
    if fail_reason:
        hard, soft = 0, 0.0
        failed_checks = [fail_reason, *failed_checks]

    result = {
        "id": str(item["id"]),
        "hard": hard,
        "soft": soft,
        "task_type": item.get("task_type", "routing"),
        "workspace": item.get("workspace", ""),
        "question": item.get("question", ""),
        "predicted_answer": answer,
        "tool_calls": tool_calls,
        "fail_reason": "; ".join(failed_checks) if failed_checks else "",
        "stats": stats,
    }
    with open(result_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    return result


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
    out = Path(out_root)
    out.mkdir(parents=True, exist_ok=True)
    tasks_dir = out / "tasks"

    def _one(item: dict) -> dict:
        return rollout_one(
            item, skill_content, tasks_dir / str(item["id"]),
            claude_bin=claude_bin,
            claude_model=claude_model,
            crystalline_bin=crystalline_bin,
            fixture_root=Path(fixture_root),
            max_turns=max_turns,
            exec_timeout=exec_timeout,
        )

    if workers > 1 and len(items) > 1:
        with ThreadPoolExecutor(max_workers=workers) as pool:
            results = list(pool.map(_one, items))
    else:
        results = [_one(item) for item in items]

    with open(out / "rollouts.json", "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=2)
    return results
