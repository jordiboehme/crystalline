"""Shared rollout plumbing for the crystalline skill benchmarks.

Every env's task runs the same way - a real headless Claude Code session
against an isolated crystalline MCP server:

1. Copy the item's fixture workspace domains into a temp sandbox.
2. Register each domain into a sandbox-private config and index
   (XDG_STATE_HOME and XDG_CONFIG_HOME point into the sandbox, so the
   single-instance lock and the state never touch the user's own
   crystalline installation).
3. Launch ``claude -p`` with the candidate skill as an appended system
   prompt, the sandbox MCP server as the only MCP config and the file
   tools disallowed so the agent cannot bypass the knowledge tools by
   touching the fixture files directly.
4. Parse the stream-json transcript into tool calls plus the final
   answer, then hand both to the env's scorer.

Env-specific behavior enters through two callbacks:

- ``prepare(item, sandbox)`` runs after sandbox setup, before the agent;
  its return value is passed through to the scorer (capture snapshots
  the pre-run verify report and file list here).
- ``score(item, sandbox, tool_calls, answer, prepared)`` runs while the
  sandbox is still alive, so post-state checks can read the domain
  files. It returns ``(hard, soft, failed_check_descriptions)``.

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
from typing import Any, Callable

DISALLOWED_TOOLS = [
    "Bash", "Read", "Grep", "Glob", "Edit", "Write",
    "WebSearch", "WebFetch", "Task", "NotebookEdit", "TodoWrite",
]

TRANSIENT_API_STATUS = {401, 403, 429, 500, 502, 503, 529}

PrepareFn = Callable[[dict, Path], Any]
ScoreFn = Callable[[dict, Path, list[dict], str, Any], tuple[int, float, list[str]]]


class TransientRolloutError(RuntimeError):
    """The session failed for infrastructure reasons (rate limit, auth,
    API outage), not agent behavior. Such a task must never be scored or
    cached: raising aborts the batch loudly and a later re-run of the
    same command resumes past the tasks that did complete."""


def detect_transient(events: list[dict]) -> str:
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


def run_cmd(cmd: list[str], env: dict, timeout: int) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd, capture_output=True, text=True, encoding="utf-8",
        errors="replace", timeout=timeout, env=env,
    )


def sandbox_env(sandbox: Path) -> dict:
    return dict(
        os.environ,
        XDG_STATE_HOME=str(sandbox / "state"),
        XDG_CONFIG_HOME=str(sandbox / "xdg-config"),
    )


def setup_sandbox(
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
    env = sandbox_env(sandbox)
    for domain_dir in sorted(domains_dst.iterdir()):
        if not domain_dir.is_dir():
            continue
        proc = run_cmd(
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


def tool_result_text(block: dict) -> str:
    content = block.get("content")
    if isinstance(content, str):
        return content
    parts = []
    for part in content or []:
        if isinstance(part, dict) and part.get("type") == "text":
            parts.append(str(part.get("text", "")))
    return "\n".join(parts)


def parse_transcript(
    raw: str,
) -> tuple[list[dict], str, dict, list[dict], list[dict]]:
    """Extract (tool_calls, final_answer, stats, events, conversation).

    The conversation is the analyst-facing trajectory: assistant text as
    role/content entries and each tool call as a tool_call record whose
    observation is filled in from the matching tool_result block.
    """
    tool_calls: list[dict] = []
    answer = ""
    stats: dict = {}
    events: list[dict] = []
    conversation: list[dict] = []
    obs_by_id: dict[str, dict] = {}
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
                    call_input = block.get("input", {}) or {}
                    tool_calls.append({
                        "name": block.get("name", ""),
                        "input": call_input,
                    })
                    record = {
                        "type": "tool_call",
                        "cmd": f"{block.get('name', '')} {json.dumps(call_input, ensure_ascii=False)}",
                        "obs": "",
                    }
                    conversation.append(record)
                    if block.get("id"):
                        obs_by_id[block["id"]] = record
                elif block.get("type") == "text" and str(block.get("text", "")).strip():
                    conversation.append({
                        "role": "agent",
                        "content": str(block["text"]),
                    })
        elif event.get("type") == "user":
            for block in event.get("message", {}).get("content", []) or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    record = obs_by_id.get(str(block.get("tool_use_id", "")))
                    if record is not None:
                        record["obs"] = tool_result_text(block)
        elif event.get("type") == "result":
            answer = str(event.get("result") or "")
            stats = {
                "subtype": event.get("subtype"),
                "num_turns": event.get("num_turns"),
                "duration_ms": event.get("duration_ms"),
                "total_cost_usd": event.get("total_cost_usd"),
            }
    return tool_calls, answer, stats, events, conversation


def write_predictions(
    predictions_dir: Path,
    item: dict,
    conversation: list[dict],
    skill_content: str,
) -> None:
    """Write the per-task files the reflect stage reads: reflection skips
    any task without predictions/<id>/conversation.json."""
    task_pred = predictions_dir / str(item["id"])
    task_pred.mkdir(parents=True, exist_ok=True)
    with open(task_pred / "conversation.json", "w", encoding="utf-8") as f:
        json.dump(conversation, f, ensure_ascii=False, indent=2)
    (task_pred / "target_system_prompt.txt").write_text(
        skill_content or "(empty skill)", encoding="utf-8"
    )
    (task_pred / "target_user_prompt.txt").write_text(
        str(item.get("question", "")), encoding="utf-8"
    )


def rollout_one(
    item: dict,
    skill_content: str,
    task_dir: Path,
    predictions_dir: Path,
    *,
    claude_bin: str,
    claude_model: str,
    crystalline_bin: str,
    fixture_root: Path,
    max_turns: int,
    exec_timeout: int,
    score: ScoreFn,
    prepare: PrepareFn | None = None,
    default_task_type: str,
    sandbox_prefix: str,
) -> dict:
    result_path = task_dir / "result.json"
    if result_path.exists():
        with open(result_path, encoding="utf-8") as f:
            cached = json.load(f)
        if not (predictions_dir / str(item["id"]) / "conversation.json").exists():
            # Older cached results predate the prediction files the
            # reflect stage needs; regenerate them from the transcript.
            raw = (task_dir / "transcript.jsonl").read_text(encoding="utf-8")
            _, _, _, _, conversation = parse_transcript(raw)
            write_predictions(predictions_dir, item, conversation, skill_content)
        if "reference_text" not in cached:
            cached["task_description"] = item.get("question", "")
            cached["n_turns"] = (cached.get("stats") or {}).get("num_turns")
            cached["reference_text"] = item.get("reference_text", "")
            with open(result_path, "w", encoding="utf-8") as f:
                json.dump(cached, f, ensure_ascii=False, indent=2)
        return cached

    task_dir.mkdir(parents=True, exist_ok=True)
    fixture_dir = fixture_root / str(item["workspace"])
    if not fixture_dir.is_dir():
        raise FileNotFoundError(f"fixture workspace missing: {fixture_dir}")

    sandbox = Path(tempfile.mkdtemp(prefix=sandbox_prefix))
    fail_reason = ""
    try:
        mcp_config = setup_sandbox(sandbox, fixture_dir, crystalline_bin)
        prepared = prepare(item, sandbox) if prepare else None

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
        tool_calls, answer, stats, events, conversation = parse_transcript(raw)

        transient = detect_transient(events)
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

        # Score while the sandbox is alive so post-state checks can read
        # the domain files the agent may have written.
        hard, soft, failed_checks = score(item, sandbox, tool_calls, answer, prepared)
    finally:
        shutil.rmtree(sandbox, ignore_errors=True)

    if fail_reason:
        hard, soft = 0, 0.0
        failed_checks = [fail_reason, *failed_checks]

    result = {
        "id": str(item["id"]),
        "hard": hard,
        "soft": soft,
        "task_type": item.get("task_type", default_task_type),
        "workspace": item.get("workspace", ""),
        "question": item.get("question", ""),
        "task_description": item.get("question", ""),
        "n_turns": stats.get("num_turns"),
        "reference_text": item.get("reference_text", ""),
        "predicted_answer": answer,
        "tool_calls": tool_calls,
        "fail_reason": "; ".join(failed_checks) if failed_checks else "",
        "stats": stats,
    }
    write_predictions(predictions_dir, item, conversation, skill_content)
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
    workers: int,
    max_turns: int,
    exec_timeout: int,
    score: ScoreFn,
    prepare: PrepareFn | None = None,
    default_task_type: str,
    sandbox_prefix: str,
) -> list[dict]:
    out = Path(out_root)
    out.mkdir(parents=True, exist_ok=True)
    tasks_dir = out / "tasks"
    predictions_dir = out / "predictions"

    def _one(item: dict) -> dict:
        return rollout_one(
            item, skill_content, tasks_dir / str(item["id"]), predictions_dir,
            claude_bin=claude_bin,
            claude_model=claude_model,
            crystalline_bin=crystalline_bin,
            fixture_root=Path(fixture_root),
            max_turns=max_turns,
            exec_timeout=exec_timeout,
            score=score,
            prepare=prepare,
            default_task_type=default_task_type,
            sandbox_prefix=sandbox_prefix,
        )

    if workers > 1 and len(items) > 1:
        with ThreadPoolExecutor(max_workers=workers) as pool:
            results = list(pool.map(_one, items))
    else:
        results = [_one(item) for item in items]

    with open(out / "rollouts.json", "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=2)
    return results


# ── Sandbox post-state helpers (verify snapshots, frontmatter) ────────────

def snapshot(sandbox: Path, crystalline_bin: str) -> dict:
    """Capture the domain state: verify issues and the file listing.

    ``verify_errors`` keeps the capture benchmark's original shape (a set
    of ``(path, rule)`` for error-severity issues); ``verify_issues``
    carries every severity as ``(path, rule, severity)`` for envs that
    need to track warnings too.
    """
    domains_root = sandbox / "domains"
    errors: set[tuple[str, str]] = set()
    issues: set[tuple[str, str, str]] = set()
    for domain_dir in sorted(p for p in domains_root.iterdir() if p.is_dir()):
        proc = run_cmd(
            [crystalline_bin, "verify", str(domain_dir), "--format", "json"],
            env=sandbox_env(sandbox), timeout=60,
        )
        if proc.returncode not in (0, 1):
            raise RuntimeError(
                f"verify failed on {domain_dir.name}: {proc.stderr.strip()}"
            )
        report = json.loads(proc.stdout or "{}")
        for issue in report.get("issues", []):
            severity = str(issue.get("severity", "")).lower()
            rel = str(issue.get("path", "")).replace("\\", "/")
            rel = rel.split("/domains/", 1)[-1]
            rule = str(issue.get("rule", ""))
            issues.add((rel, rule, severity))
            if severity == "error":
                errors.add((rel, rule))
    files = {
        str(p.relative_to(domains_root))
        for p in domains_root.rglob("*.md")
    }
    return {"verify_errors": errors, "verify_issues": issues, "files": files}


def read_frontmatter(path: Path) -> dict:
    import yaml

    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        return {}
    end = text.find("\n---\n", 4)
    if end < 0:
        return {}
    try:
        parsed = yaml.safe_load(text[4:end])
    except yaml.YAMLError:
        return {}
    return parsed if isinstance(parsed, dict) else {}


def find_engram_file(sandbox: Path, domain: str, permalink: str) -> Path | None:
    domain_dir = sandbox / "domains" / domain
    slug = permalink.strip("/").split("/")[-1]
    for p in domain_dir.rglob("*.md"):
        if p.stem == slug:
            return p
    return None
