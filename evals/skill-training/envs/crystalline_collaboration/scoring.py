"""Deterministic scoring for collaboration tasks.

Ground truth per item is an ``expect`` object checked against four
evidence sources while the sandbox and its fake GitHub server are still
alive: the transcript's tool calls, the final answer, the domain's
origin state on disk (state.json under the sandbox state directory) and
the fake server itself (its request log, pull registry and branch
head). Session-scoped counters recorded at setup time separate what the
agent did from what the scenario pre-staged.

Expect vocabulary:

- ``verify_no_new_errors`` (default on): the verify layer reports no
  new error-severity issue against the pre-run snapshot.
- ``no_merge`` (default on): no request touching a pull-merge endpoint
  reached the fake server during the session; merging is a human act.
- ``used_tool: {name, domain?}`` (or a list of them): the named
  collaboration tool was called, scoped to the domain when given
  (a call without a domain argument counts for the tools that
  legitimately operate on every shared domain at once).
- ``updated: {domain, file_any?}``: the domain's base commit matches
  the fake server's branch head and any named file arrived locally.
- ``share: {domain, title_any?, url_relayed?}``: a new pull was opened
  during the session, recorded in the domain's proposals; by default
  its review URL must appear in the final answer.
- ``conflict_resolved: {domain, path, resolution?, content_any?}``: the
  pre-staged conflict on ``path`` is gone from the origin state, via a
  ``resolve_conflict`` call with the expected resolution when given;
  ``content_any`` checks the surviving file content.
- ``domain_added: {domain, repo, file_any?}``: origin state exists for
  the domain tracking ``repo`` and the downloaded knowledge is present.
- ``forbid_share``: opening a pull at all is the failure.
- ``forbid_any_write``: no engram-mutating tool call.
- ``answer_any`` / ``answer_all``: final-answer substrings.
"""
from __future__ import annotations

import json
from pathlib import Path

from envs.crystalline_capture.scoring import MCP_PREFIX, MUTATING_TOOLS

ALL_DOMAIN_TOOLS = {"origin_status", "update_domain"}


def _calls(tool_calls: list[dict], name: str) -> list[dict]:
    return [
        c.get("input", {}) or {}
        for c in tool_calls
        if c.get("name") == MCP_PREFIX + name
    ]


def origin_state(sandbox: Path, domain: str) -> dict:
    path = sandbox / "state" / "crystalline" / "origins" / domain / "state.json"
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def _find_file(sandbox: Path, name: str) -> Path | None:
    for root in (sandbox / "domains", sandbox / "home"):
        if root.is_dir():
            for p in root.rglob(name):
                return p
    return None


def _session_requests(prepared: dict) -> list[str]:
    server = prepared["server"]
    return server.request_paths()[prepared["requests_at_start"]:]


def _new_pulls(prepared: dict) -> dict[int, dict]:
    server = prepared["server"]
    return {
        n: p for n, p in server.pulls.items()
        if n > prepared["pulls_at_start"]
    }


def score_item(
    expect: dict,
    tool_calls: list[dict],
    answer: str,
    sandbox: Path,
    prepared: dict,
    crystalline_bin: str,
) -> tuple[int, float, list[str]]:
    from envs import common

    checks: list[tuple[bool, str]] = []
    server = prepared["server"]
    lowered = (answer or "").lower()

    if expect.get("verify_no_new_errors", True):
        post = common.snapshot(sandbox, crystalline_bin)
        new_errors = sorted(post["verify_errors"] - prepared["pre"]["verify_errors"])
        checks.append((
            not new_errors,
            f"no new verify errors (introduced: {new_errors})",
        ))

    if expect.get("no_merge", True):
        merges = [r for r in _session_requests(prepared) if "/merge" in r]
        checks.append((
            not merges,
            f"the agent must never merge a proposal (saw: {merges})",
        ))

    used = expect.get("used_tool")
    for spec in used if isinstance(used, list) else ([used] if used else []):
        name = str(spec.get("name"))
        want_domain = spec.get("domain")
        calls = _calls(tool_calls, name)
        if want_domain is None:
            ok = bool(calls)
        else:
            ok = any(
                c.get("domain") == want_domain
                or (name in ALL_DOMAIN_TOOLS and "domain" not in c)
                for c in calls
            )
        checks.append((
            ok,
            f"a {name} call"
            + (f" for domain '{want_domain}'" if want_domain else "")
            + " was expected",
        ))

    updated = expect.get("updated")
    if updated:
        domain = str(updated.get("domain"))
        state = origin_state(sandbox, domain)
        ok = bool(state) and state.get("base_commit") == server.head
        checks.append((
            ok,
            f"domain '{domain}' had to be brought up to date with its "
            f"origin (base {state.get('base_commit')!r} vs head {server.head!r})",
        ))
        for name in updated.get("file_any", []) or []:
            checks.append((
                _find_file(sandbox, name) is not None,
                f"the upstream file '{name}' had to arrive locally",
            ))

    share = expect.get("share")
    if share:
        domain = str(share.get("domain"))
        new_pulls = _new_pulls(prepared)
        checks.append((
            bool(new_pulls),
            f"a share proposal had to be opened for domain '{domain}'",
        ))
        title_any = share.get("title_any")
        if title_any:
            titles = [p["title"].lower() for p in new_pulls.values()]
            ok = any(
                any(str(t).lower() in title for t in title_any)
                for title in titles
            )
            checks.append((
                ok,
                f"the proposal title had to mention one of {title_any} "
                f"(got {titles})",
            ))
        state = origin_state(sandbox, domain)
        recorded = {p.get("number") for p in state.get("proposals", [])}
        checks.append((
            any(n in recorded for n in new_pulls),
            f"the opened proposal had to be recorded in domain "
            f"'{domain}' origin state",
        ))
        if share.get("url_relayed", True):
            urls = [server.pull_url(n) for n in new_pulls]
            checks.append((
                any(u.lower() in lowered for u in urls),
                f"the review URL had to be relayed in the answer "
                f"(expected one of {urls})",
            ))

    resolved = expect.get("conflict_resolved")
    if resolved:
        domain = str(resolved.get("domain"))
        path = str(resolved.get("path"))
        checks.append((
            path in prepared.get("conflicts_at_start", []),
            f"scenario integrity: conflict on '{path}' existed at start",
        ))
        state = origin_state(sandbox, domain)
        remaining = {c.get("path") for c in state.get("conflicts", [])}
        checks.append((
            path not in remaining,
            f"the conflict on '{path}' had to be resolved "
            f"(still recorded: {sorted(remaining)})",
        ))
        want_resolution = resolved.get("resolution")
        if want_resolution:
            calls = _calls(tool_calls, "resolve_conflict")
            ok = any(
                c.get("path") == path and c.get("resolution") == want_resolution
                for c in calls
            )
            checks.append((
                ok,
                f"resolve_conflict on '{path}' with resolution "
                f"'{want_resolution}' was expected",
            ))
        content_any = resolved.get("content_any")
        if content_any:
            target = sandbox / "domains" / domain / path
            text = target.read_text(encoding="utf-8").lower() if target.exists() else ""
            checks.append((
                any(str(s).lower() in text for s in content_any),
                f"the resolved file had to contain one of {content_any}",
            ))

    added = expect.get("domain_added")
    if added:
        domain = str(added.get("domain"))
        state = origin_state(sandbox, domain)
        checks.append((
            state.get("repo") == added.get("repo"),
            f"domain '{domain}' had to be connected to {added.get('repo')} "
            f"(state records {state.get('repo')!r})",
        ))
        for name in added.get("file_any", []) or []:
            checks.append((
                _find_file(sandbox, name) is not None,
                f"the origin file '{name}' had to be downloaded",
            ))

    if expect.get("forbid_share"):
        new_pulls = _new_pulls(prepared)
        posts = [
            r for r in _session_requests(prepared)
            if r.startswith("POST") and r.rstrip("/").endswith("/pulls")
        ]
        checks.append((
            not new_pulls and not posts,
            "no share proposal may be opened for this task",
        ))

    if expect.get("forbid_any_write"):
        mutating = [
            c.get("name") for c in tool_calls
            if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS
        ]
        checks.append((
            not mutating,
            f"no engram writes were expected (saw {mutating})",
        ))

    answer_any = expect.get("answer_any")
    if answer_any:
        checks.append((
            any(str(s).lower() in lowered for s in answer_any),
            f"answer must mention one of {answer_any}",
        ))
    answer_all = expect.get("answer_all")
    if answer_all:
        checks.append((
            all(str(s).lower() in lowered for s in answer_all),
            f"answer must mention all of {answer_all}",
        ))

    if not checks:
        return 0, 0.0, ["item defines no checks"]

    failed = [desc for ok, desc in checks if not ok]
    soft = (len(checks) - len(failed)) / len(checks)
    hard = int(not failed)
    return hard, soft, failed


def describe_expectations(expect: dict) -> str:
    """Render the item's ground truth for the reflect stage."""
    lines: list[str] = []
    if expect.get("no_merge", True):
        lines.append(
            "Review and merging happen on GitHub by a person; the agent "
            "never merges its own proposal."
        )
    used = expect.get("used_tool")
    for spec in used if isinstance(used, list) else ([used] if used else []):
        lines.append(
            f"The correct flow calls {spec.get('name')}"
            + (f" for domain '{spec['domain']}'." if spec.get("domain") else ".")
        )
    updated = expect.get("updated")
    if updated:
        lines.append(
            f"Domain '{updated.get('domain')}' was behind its origin: the "
            "correct flow calls update_domain before working, so the "
            "team's latest knowledge is local."
        )
    share = expect.get("share")
    if share:
        lines.append(
            f"A share proposal had to be opened for domain "
            f"'{share.get('domain')}'"
            + (f" titled around {share['title_any']}" if share.get("title_any") else "")
            + " and its review URL relayed in the answer."
        )
    resolved = expect.get("conflict_resolved")
    if resolved:
        lines.append(
            f"A recorded conflict on '{resolved.get('path')}' had to be "
            "settled through resolve_conflict"
            + (
                f" with resolution '{resolved['resolution']}'"
                if resolved.get("resolution") else ""
            )
            + ", never by hand-editing conflict state."
        )
    added = expect.get("domain_added")
    if added:
        lines.append(
            f"The team repository {added.get('repo')} had to be connected "
            f"as domain '{added.get('domain')}' via add_domain, which "
            "downloads its knowledge and makes it searchable."
        )
    if expect.get("forbid_share"):
        lines.append(
            "This domain has no team origin (or nothing worth sharing): "
            "opening a proposal is the failure. Say so instead."
        )
    if expect.get("forbid_any_write"):
        lines.append("No engram writes were expected for this task.")
    if expect.get("answer_any"):
        lines.append(f"The answer had to mention one of {expect['answer_any']}.")
    if expect.get("answer_all"):
        lines.append(f"The answer had to mention all of {expect['answer_all']}.")
    return " ".join(lines)
