"""Collaboration benchmark rollout: the shared sandboxed Claude Code
runner with a per-item fake GitHub server and scenario staging.

Each sandbox gets its own in-process GitHub stand-in (fake_github.py)
seeded from a fixture origin tree, then the scenario is staged with the
real binary: the origin domain is connected with `domain add --origin`,
local edits are written to disk, upstream commits land server-side and
a conflict or a pre-opened (possibly declined) proposal is materialized
through the same CLI verbs an operator would use. The agent session
then runs with the collaboration tools enabled and pointed at the fake
server; scoring reads the transcript, the origin state on disk and the
server's own registry and request log.
"""
from __future__ import annotations

from pathlib import Path

from envs import common
from envs.common import TransientRolloutError  # noqa: F401  (re-export)
from envs.crystalline_collaboration.fake_github import FakeGitHub
from envs.crystalline_collaboration.scoring import origin_state, score_item

PLACEHOLDER_TREE = {
    "MANIFEST.md": (
        "---\n"
        "type: manifest\n"
        "title: placeholder\n"
        "permalink: manifest\n"
        "tags:\n"
        "  - manifest\n"
        "status: current\n"
        "---\n\n"
        "# placeholder\n\n"
        "## Scope\n\n"
        "- An empty team repository nothing in the scenario connects to\n\n"
        "## When to Use\n\n"
        "- Never; it exists so the fake server always has a repository\n"
    ),
}


def _read_tree(tree_dir: Path) -> dict[str, str]:
    files: dict[str, str] = {}
    for path in sorted(tree_dir.rglob("*")):
        if path.is_file():
            rel = path.relative_to(tree_dir).as_posix()
            files[rel] = path.read_text(encoding="utf-8")
    return files


def _apply_delta(base: str | None, delta) -> str | None:
    """One file's scenario delta: None deletes, ``content`` replaces the
    whole file, ``replace`` rewrites a unique substring of the base."""
    if delta is None:
        return None
    if "content" in delta:
        return str(delta["content"])
    old, new = delta["replace"]
    if base is None or old not in base:
        raise RuntimeError(f"scenario replace target not found: {old!r}")
    return base.replace(old, new)


def setup_collab_sandbox(
    item: dict,
    sandbox: Path,
    fixture_root: Path,
    crystalline_bin: str,
) -> tuple[Path, dict]:
    fixture_dir = fixture_root / str(item["workspace"])
    spec = item.get("origin") or {}
    tree_name = spec.get("tree")
    tree = (
        _read_tree(fixture_dir / "origins" / str(tree_name))
        if tree_name else dict(PLACEHOLDER_TREE)
    )
    server = FakeGitHub(str(spec.get("repo", "acme/placeholder")), tree)
    try:
        base_url = server.start()
        (sandbox / "home").mkdir()
        (sandbox / "cache").mkdir()
        extra_env = {
            "HOME": str(sandbox / "home"),
            "XDG_CACHE_HOME": str(sandbox / "cache"),
            "CRYSTALLINE_GITHUB_ENABLED": "true",
            "CRYSTALLINE_GITHUB_TOKEN": "sandbox-token",
            "CRYSTALLINE_GITHUB_API_URL": base_url,
        }
        mcp_config = common.setup_sandbox(
            sandbox, fixture_dir, crystalline_bin, extra_env=extra_env
        )
        env = common.sandbox_env(sandbox)
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
                    f"scenario staging '{args[0]} {args[1] if len(args) > 1 else ''}' "
                    f"failed: {proc.stderr.strip()}"
                )

        domain = str(spec.get("domain") or "")
        staged = False
        if domain and spec.get("preconnect", True):
            # Adopt the already-registered fixture domain in place. The root
            # must be spelled the way the engine stored it when common.setup
            # registered it, which is the symlink-resolved path (on macOS the
            # sandbox lives under /var -> /private/var); passing the raw path
            # would fail the adopt-in-place check as a different root.
            cli(
                "domain", "add", domain,
                str((sandbox / "domains" / domain).resolve()),
                "--origin", str(spec["repo"]),
            )
        for rel, delta in (spec.get("local_edits") or {}).items():
            target = sandbox / "domains" / domain / rel
            base = target.read_text(encoding="utf-8") if target.exists() else None
            new = _apply_delta(base, delta)
            if new is None:
                target.unlink()
            else:
                target.write_text(new, encoding="utf-8")
            staged = True
        upstream = spec.get("upstream_commit")
        if upstream:
            head_files = {
                path: content.decode("utf-8")
                for path, content in server.head_files().items()
            }
            changes = {
                rel: _apply_delta(head_files.get(rel), delta)
                for rel, delta in upstream.items()
            }
            server.commit_upstream(changes)
        if spec.get("materialize_conflict"):
            cli("origin", "update", "--domain", domain)
            staged = True
        if spec.get("share_first"):
            cli("origin", "share", domain, "--title", str(spec["share_first"]))
            if spec.get("close_share"):
                server.close_pull(max(server.pulls))
        if staged:
            cli("sync")

        state = origin_state(sandbox, domain) if domain else {}
        prepared = {
            "server": server,
            "pre": common.snapshot(sandbox, crystalline_bin),
            "pulls_at_start": max(server.pulls, default=0),
            "requests_at_start": len(server.requests),
            "conflicts_at_start": [
                str(c.get("path")) for c in state.get("conflicts", [])
            ],
        }
        return mcp_config, prepared
    except Exception:
        server.stop()
        raise


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
    fixture_root_path = Path(fixture_root)

    def _setup(item: dict, sandbox: Path) -> tuple[Path, dict]:
        return setup_collab_sandbox(
            item, sandbox, fixture_root_path, crystalline_bin
        )

    def _teardown(prepared) -> None:
        if isinstance(prepared, dict) and prepared.get("server"):
            prepared["server"].stop()

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
        setup=_setup,
        teardown=_teardown,
        default_task_type="collaboration",
        sandbox_prefix="cst-collab-",
    )
