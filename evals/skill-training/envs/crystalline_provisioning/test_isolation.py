"""HOME isolation: a provisioning run in the sandbox writes under the
sandbox HOME and nothing under the real one.

Needs a built crystalline binary: CRYSTALLINE_BIN, else the release build
of this checkout. Skipped when neither exists.

    CRYSTALLINE_BIN=/path/to/crystalline uv run --with pytest pytest \\
        envs/crystalline_provisioning/test_isolation.py
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from envs import common
from envs.crystalline_provisioning import rollout

HARNESS_ROOT = Path(__file__).resolve().parents[2]
BIN = os.environ.get("CRYSTALLINE_BIN") or str(
    HARNESS_ROOT.parent.parent / "target" / "release" / "crystalline"
)
PROBE_MCP = "isolation-probe-mcp"

pytestmark = pytest.mark.skipif(
    not Path(BIN).is_file(), reason=f"no crystalline binary at {BIN}"
)


def _real_mcp_names() -> set[str]:
    path = Path.home() / ".claude.json"
    try:
        return set(json.loads(path.read_text(encoding="utf-8")).get("mcpServers", {}))
    except (OSError, json.JSONDecodeError):
        return set()


def test_provisioning_writes_only_under_the_sandbox_home(tmp_path):
    before = rollout.guard_digest()
    real_before = _real_mcp_names()
    assert PROBE_MCP not in real_before

    sandbox = tmp_path / "sb"
    sandbox.mkdir()
    item = {"id": "isolation", "workspace": "provisioning",
            "stage": {"declare": ["skills", "mcps"]}}
    _, prepared = rollout.setup_provisioning_sandbox(
        item, sandbox, HARNESS_ROOT / "fixtures", BIN
    )
    try:
        mcps = sandbox / "domains" / "harbor-tools" / "mcps"
        mcps.mkdir()
        (mcps / f"{PROBE_MCP}.json").write_text(json.dumps(
            {"name": PROBE_MCP, "server": {"command": "true", "args": []}}
        ))
        env = common.sandbox_env(sandbox)
        env.update(common.home_env(sandbox))
        env["CRYSTALLINE_TEST_NO_KEYCHAIN"] = "1"
        config = str(sandbox / "config.yaml")

        # The decision and its apply, as the staging and the MCP server run it.
        proc = common.run_cmd(
            [BIN, "provision", "allow", "harbor", "--config", config], env=env, timeout=120
        )
        assert proc.returncode == 0, proc.stderr
        # A bare apply from the agent's shell, through the session env file
        # the Bash tool sources, with the real HOME in the parent process.
        shell = subprocess.run(
            ["bash", "-c", f'source "{sandbox / "session-env.sh"}" && echo "$HOME" && crystalline provision'],
            capture_output=True, text=True, timeout=120,
            env={k: v for k, v in os.environ.items() if k != "CRYSTALLINE_CONFIG"},
        )
        assert shell.returncode == 0, shell.stderr
        assert shell.stdout.splitlines()[0] == str(sandbox / "home")

        installed = sandbox / "home" / ".claude" / "skills" / "pilotage-checklist" / "SKILL.md"
        assert installed.is_file(), "the provisioned skill must land in the sandbox HOME"
        print(f"sandbox install: {installed}")
        print(f"allow output: {proc.stdout.strip()}")
        print(f"apply output: {shell.stdout.strip()}")
        if shutil.which("claude"):
            sandbox_cfg = json.loads((sandbox / "home" / ".claude.json").read_text())
            assert PROBE_MCP in sandbox_cfg.get("mcpServers", {}), (
                "the provisioned MCP server must be registered in the sandbox HOME"
            )
    finally:
        rollout.stop_sandbox_daemon(sandbox)
        if prepared.get("server"):
            prepared["server"].stop()

    after = rollout.guard_digest()
    print(f"real harness folders before: {before}")
    print(f"real harness folders after:  {after}")
    assert after == before, "the real ~/.claude skills, commands or agents changed"
    assert PROBE_MCP not in _real_mcp_names(), "the real ~/.claude.json gained the probe server"
    assert not (Path.home() / ".claude" / "skills" / "pilotage-checklist").exists()
