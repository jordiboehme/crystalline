"""Provisioning benchmark rollout: the shared sandboxed Claude Code runner
with an isolated HOME, a seeded install receipt and a shell.

Provisioning writes into the person's home (skills under
``~/.claude/skills``, MCP servers through ``claude mcp add-json`` into
``~/.claude.json``), so every process that can provision runs with HOME
inside the sandbox:

- the MCP server and the staging CLI calls get ``HOME``, ``COPILOT_HOME``
  and ``CODEX_HOME`` from ``common.home_env`` (``isolate_home=True``);
- the agent's Bash tool sources ``<sandbox>/session-env.sh`` before each
  command (``CLAUDE_ENV_FILE``), which moves HOME, the XDG folders and
  the crystalline config into the sandbox and puts the benchmark's own
  ``crystalline`` binary first on PATH;
- the ``claude`` process itself keeps the real HOME for its login, so
  the file tools that resolve paths in that process (Read, Write, Edit,
  Glob, Grep) stay disallowed and every file operation goes through Bash.

The fixture workspace ``provisioning`` holds the local domain ``harbor``
(no Provisioning section), the tools repo ``harbor-tools`` that items
place beside it, a pulled change for the update items, the person's
harness home (three candidate skills and an MCP registration carrying the
fake token ``tok-FIXTURE-123``) and the team repository ``harbor/ops``
whose ``ops/`` subfolder is the team domain ``fieldops``, served by the
collaboration benchmark's fake GitHub server.

Item ``stage`` keys: ``tools_repo`` (default true) copies the tools repo
beside the domain; ``declare`` appends a Provisioning section with those
kinds; ``allow`` records the allow decision through the CLI (which also
applies it); ``pull`` drops the pulled change into the tools repo after
that; ``team`` connects ``fieldops``.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import signal
import sys
from pathlib import Path

from envs import common
from envs.common import TransientRolloutError  # noqa: F401  (re-export)
from envs.crystalline_collaboration.fake_github import FakeGitHub
from envs.crystalline_provisioning.scoring import TOOLS_REPO, score_item

ALLOWED_TOOLS = ["mcp__crystalline", "Bash"]
# The only built-in tool offered: everything else either resolves paths
# with the real HOME or reaches outside the sandbox.
BUILTIN_TOOLS = ["Bash"]
DISALLOWED_TOOLS = [
    "Read", "Grep", "Glob", "Edit", "Write", "NotebookEdit",
    "WebSearch", "WebFetch", "Task", "TodoWrite",
]
TEAM_REPO = "harbor/ops"
TEAM_SUBPATH = "ops"
TEAM_DOMAIN = "fieldops"
# The real harness folders a provisioning run could reach if isolation
# failed; checksummed around every batch.
GUARDED = [".claude/skills", ".claude/commands", ".claude/agents"]


def binary_version(crystalline_bin: str) -> str:
    proc = common.run_cmd([crystalline_bin, "--version"], env=dict(os.environ), timeout=30)
    lines = (proc.stdout or "").splitlines()
    words = lines[0].split() if lines else []
    if proc.returncode != 0 or len(words) < 2:
        raise RuntimeError(f"crystalline --version failed: {proc.stderr.strip()}")
    return words[1]


def seed_receipt(sandbox: Path, version: str) -> Path:
    """An install receipt naming Claude Code at this binary's version, so
    provisioning has a target and nothing auto-reconciles."""
    path = sandbox / "state" / "crystalline" / "installs.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    row = {
        "harness": "claude-code",
        "scope": "user",
        "version": version,
        "parts": {"mcp": True, "hooks": False, "skills": True},
        "skills": [],
    }
    path.write_text(json.dumps({"format": 1, "installs": [row]}), encoding="utf-8")
    return path


def _read_tree(tree_dir: Path) -> dict[str, str]:
    return {
        p.relative_to(tree_dir).as_posix(): p.read_text(encoding="utf-8")
        for p in sorted(tree_dir.rglob("*"))
        if p.is_file()
    }


def write_session_env(sandbox: Path, crystalline_bin: str, extra: dict) -> Path:
    """The file the agent's Bash tool sources before every command."""
    bin_dir = sandbox / "bin"
    bin_dir.mkdir(exist_ok=True)
    link = bin_dir / "crystalline"
    if not link.exists():
        link.symlink_to(Path(crystalline_bin).resolve())
    values = {
        **common.home_env(sandbox),
        "XDG_STATE_HOME": str(sandbox / "state"),
        "XDG_CONFIG_HOME": str(sandbox / "xdg-config"),
        "CRYSTALLINE_CONFIG": str(sandbox / "config.yaml"),
        "CRYSTALLINE_TEST_NO_KEYCHAIN": "1",
        **extra,
    }
    lines = [f"export {k}={json.dumps(v)}" for k, v in values.items()]
    lines.append(f'export PATH={json.dumps(str(bin_dir))}":$PATH"')
    path = sandbox / "session-env.sh"
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def setup_provisioning_sandbox(
    item: dict,
    sandbox: Path,
    fixture_root: Path,
    crystalline_bin: str,
) -> tuple[Path, dict]:
    fixture_dir = fixture_root / str(item["workspace"])
    stage = item.get("stage") or {}
    server = None
    try:
        shutil.copytree(fixture_dir / "home", sandbox / "home", symlinks=True)
        seed_receipt(sandbox, binary_version(crystalline_bin))
        extra_env = {
            "XDG_CACHE_HOME": str(sandbox / "cache"),
            "CRYSTALLINE_TEST_NO_KEYCHAIN": "1",
        }
        (sandbox / "cache").mkdir()
        if stage.get("team"):
            server = FakeGitHub(TEAM_REPO, _read_tree(fixture_dir / "origins" / "ops"))
            extra_env.update({
                "CRYSTALLINE_GITHUB_ENABLED": "true",
                "CRYSTALLINE_GITHUB_TOKEN": "sandbox-token",
                "CRYSTALLINE_GITHUB_API_URL": server.start(),
            })
        mcp_config = common.setup_sandbox(
            sandbox, fixture_dir, crystalline_bin,
            extra_env=extra_env, isolate_home=True,
        )
        env = common.sandbox_env(sandbox)
        env.update(common.home_env(sandbox))
        env.update(extra_env)
        config = sandbox / "config.yaml"
        db = sandbox / "index.db"

        def cli(*args: str) -> None:
            proc = common.run_cmd(
                [crystalline_bin, "--db", str(db), *args, "--config", str(config)],
                env=env, timeout=120,
            )
            if proc.returncode != 0:
                raise RuntimeError(
                    f"staging 'crystalline {' '.join(args[:2])}' failed: "
                    f"{proc.stderr.strip()}"
                )

        domains = sandbox / "domains"
        tools = domains / TOOLS_REPO
        if stage.get("tools_repo", True):
            shutil.copytree(fixture_dir / "tools" / TOOLS_REPO, tools)
        declare = stage.get("declare") or []
        if declare:
            manifest = domains / "harbor" / "MANIFEST.md"
            bullets = "".join(f"- {k}: ../{TOOLS_REPO}/{k}\n" for k in declare)
            text = manifest.read_text(encoding="utf-8").rstrip("\n")
            manifest.write_text(f"{text}\n\n## Provisioning\n\n{bullets}", encoding="utf-8")
            cli("sync")
        if stage.get("allow"):
            # Records the decision and applies it into the sandboxed HOME.
            proc = common.run_cmd(
                [crystalline_bin, "provision", "allow", "harbor", "--config", str(config)],
                env=env, timeout=120,
            )
            if proc.returncode != 0:
                raise RuntimeError(f"staging provision allow failed: {proc.stderr.strip()}")
        if stage.get("pull"):
            shutil.copytree(fixture_dir / "pulled" / TOOLS_REPO, tools, dirs_exist_ok=True)
        if stage.get("team"):
            cli(
                "domain", "add", TEAM_DOMAIN, str((domains / TEAM_DOMAIN).resolve()),
                "--origin", f"{TEAM_REPO}/{TEAM_SUBPATH}", "--branch", "main",
            )
        session_extra = {
            k: v for k, v in extra_env.items() if k.startswith("CRYSTALLINE_GITHUB")
        }
        write_session_env(sandbox, crystalline_bin, session_extra)
        return mcp_config, {"server": server, "sandbox": sandbox}
    except Exception:
        if server is not None:
            server.stop()
        raise


def stop_sandbox_daemon(sandbox: Path) -> None:
    """A CLI call the agent made may have spawned a daemon on the sandbox
    state folder; it would outlive the sandbox, so stop it."""
    info = sandbox / "state" / "crystalline" / "service.json"
    try:
        pid = int(json.loads(info.read_text(encoding="utf-8")).get("pid"))
    except (OSError, ValueError, TypeError, json.JSONDecodeError):
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass


def guard_digest(home: Path | None = None) -> dict[str, str]:
    """Checksums of the real harness folders provisioning could write to."""
    home = home or Path.home()
    out = {}
    for rel in GUARDED:
        root = home / rel
        h = hashlib.sha256()
        if root.exists():
            for p in sorted(root.rglob("*")):
                h.update(p.relative_to(root).as_posix().encode())
                if p.is_file() and not p.is_symlink():
                    h.update(hashlib.sha256(p.read_bytes()).digest())
        out[rel] = h.hexdigest()
    return out


def session_env(sandbox: Path) -> dict:
    return {"CLAUDE_ENV_FILE": str(sandbox / "session-env.sh")}


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
    max_turns: int = 20,
    exec_timeout: int = 300,
) -> list[dict]:
    fixture_root_path = Path(fixture_root)

    def _setup(item: dict, sandbox: Path) -> tuple[Path, dict]:
        return setup_provisioning_sandbox(item, sandbox, fixture_root_path, crystalline_bin)

    def _teardown(prepared) -> None:
        if not isinstance(prepared, dict):
            return
        if prepared.get("server"):
            prepared["server"].stop()
        if prepared.get("sandbox"):
            stop_sandbox_daemon(prepared["sandbox"])

    def _score(item, sandbox, tool_calls, answer, prepared):
        return score_item(item.get("expect", {}) or {}, tool_calls, answer, sandbox, crystalline_bin)

    before = guard_digest()
    try:
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
            setup=_setup,
            teardown=_teardown,
            session_env=session_env,
            allowed_tools=ALLOWED_TOOLS,
            disallowed_tools=DISALLOWED_TOOLS,
            builtin_tools=BUILTIN_TOOLS,
            default_task_type="provisioning",
            sandbox_prefix="cst-prov-",
        )
    finally:
        after = guard_digest()
        changed = sorted(k for k in before if before[k] != after[k])
        Path(out_root).mkdir(parents=True, exist_ok=True)
        (Path(out_root) / "home_guard.json").write_text(
            json.dumps({"before": before, "after": after, "changed": changed}, indent=2),
            encoding="utf-8",
        )
        if changed:
            print(
                f"WARNING: real harness folders changed during the batch: {changed}",
                file=sys.stderr,
            )
