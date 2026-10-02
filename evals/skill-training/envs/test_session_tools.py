"""Every env's eval session is limited to an explicit built-in tool allow-list.

    uv run --with pytest pytest envs/test_session_tools.py
"""
from __future__ import annotations

import importlib
from unittest import mock

import pytest

from envs import common

ENVS = ["routing", "capture", "schema", "collaboration", "evolve", "memory", "provisioning"]


def _tools_arg(cmd: list[str]) -> list[str]:
    assert cmd.count("--tools") == 1, "the command must carry exactly one --tools"
    value = cmd[cmd.index("--tools") + 1]
    return [t for t in value.split(",") if t]


def _assert_safe(cmd: list[str]) -> None:
    tools = _tools_arg(cmd)
    assert not set(tools) & set(common.OUTSIDE_REACHING_TOOLS)
    assert "--disallowedTools" in cmd


def test_default_command_offers_no_builtin_tool():
    cmd = common.build_claude_command(
        claude_bin="claude", claude_model="m", question="q",
        mcp_config="mcp.json", max_turns=3, skill_path="s.md", has_skill=True,
    )
    assert _tools_arg(cmd) == []
    _assert_safe(cmd)


@pytest.mark.parametrize("env", ENVS)
def test_each_env_passes_a_safe_allow_list(env, tmp_path):
    module = importlib.import_module(f"envs.crystalline_{env}.rollout")
    captured: dict = {}

    def fake_run_batch(**kwargs):
        captured.update(kwargs)
        return []

    with mock.patch.object(common, "run_batch", fake_run_batch):
        module.run_batch(
            items=[], skill_content="", out_root=str(tmp_path), claude_bin="claude",
            claude_model="m", crystalline_bin="crystalline", fixture_root=str(tmp_path),
        )
    builtin = captured.get("builtin_tools")
    assert builtin is not None
    cmd = common.build_claude_command(
        claude_bin="claude", claude_model="m", question="q",
        mcp_config="mcp.json", max_turns=3, skill_path="s.md", has_skill=False,
        allowed_tools=captured.get("allowed_tools"),
        disallowed_tools=captured.get("disallowed_tools"),
        builtin_tools=builtin,
    )
    _assert_safe(cmd)
