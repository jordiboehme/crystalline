"""Deterministic transcript scoring for crystalline routing tasks.

Every check reads the tool calls the target agent made (parsed from the
Claude Code stream-json transcript) plus its final answer text. No LLM
judge is involved: an item defines its expectations in an ``expect``
object and each defined expectation contributes one check. ``soft`` is
the fraction of defined checks that passed, ``hard`` is 1 only when all
of them passed.

Supported ``expect`` keys:

- ``answer_any``: list of substrings; the final answer must contain at
  least one of them (case-insensitive).
- ``answer_all``: list of substrings; the final answer must contain all
  of them (case-insensitive).
- ``search``: object describing at least one required search_engrams
  call. Subkeys:
    - ``domains``: exact set of the domains argument on a matching call.
    - ``domains_omitted``: true when an all-domain sweep is expected
      (no domains argument, or an empty list).
    - ``reaches_domain``: a search must have been able to find knowledge
      in this domain: either an all-domain sweep or a scoped search that
      includes it. The fair check for counter-intuitive placement, where
      both a sweep and a well-read routing decision are correct.
    - ``status``: this status filter must appear on a matching call.
    - ``forbid_status``: true when no status filter may be used on any
      search call (history questions that need every validity state).
    - ``metadata_filters_keys``: these keys must appear in the
      metadata_filters argument of a matching call (bounded windows).
- ``forbid_metadata_filters``: no search may carry metadata_filters (a
  present-day question where window filters would exclude unbounded
  engrams).
- ``manifest_read_ok``: when false, any MANIFEST read (read_engram on a
  MANIFEST identifier, or browse_domain) is a failed check; when true
  the check passes either way (a read is allowed and may even be the
  point of the task).
- ``require_manifest_read``: a MANIFEST read must have happened (the
  task is about a domain's own structure).
- ``build_context_anchor``: a build_context call whose anchor starts
  with this prefix must exist.
- ``forbid_write``: no write_engram, edit_engram, move_engram or
  delete_engram calls may appear (routing tasks are read-only).
"""
from __future__ import annotations

from typing import Any

MCP_PREFIX = "mcp__crystalline__"

WRITE_TOOLS = {"write_engram", "edit_engram", "move_engram", "delete_engram"}


def _mcp_calls(tool_calls: list[dict], name: str) -> list[dict]:
    return [
        c.get("input", {}) or {}
        for c in tool_calls
        if c.get("name") == MCP_PREFIX + name
    ]


def _domains_of(call_input: dict) -> list[str]:
    domains = call_input.get("domains") or []
    if isinstance(domains, str):
        domains = [domains]
    return sorted(str(d) for d in domains)


def _is_manifest_read(tool_calls: list[dict]) -> bool:
    for call_input in _mcp_calls(tool_calls, "read_engram"):
        identifier = str(call_input.get("identifier", "") or "")
        if "manifest" in identifier.lower():
            return True
    return bool(_mcp_calls(tool_calls, "browse_domain"))


def score_item(
    expect: dict[str, Any],
    tool_calls: list[dict],
    answer: str,
) -> tuple[int, float, list[str]]:
    """Return (hard, soft, failed_check_descriptions)."""
    checks: list[tuple[bool, str]] = []
    searches = _mcp_calls(tool_calls, "search_engrams")

    lowered = (answer or "").lower()
    answer_any = expect.get("answer_any")
    if answer_any:
        ok = any(str(s).lower() in lowered for s in answer_any)
        checks.append((ok, f"answer must mention one of {answer_any}"))
    answer_all = expect.get("answer_all")
    if answer_all:
        ok = all(str(s).lower() in lowered for s in answer_all)
        checks.append((ok, f"answer must mention all of {answer_all}"))

    search_expect = expect.get("search")
    if search_expect:
        want_domains = search_expect.get("domains")
        if want_domains is not None:
            want = sorted(str(d) for d in want_domains)
            ok = any(_domains_of(c) == want for c in searches)
            checks.append((ok, f"search_engrams scoped to domains {want}"))
        if search_expect.get("domains_omitted"):
            ok = any(not _domains_of(c) for c in searches)
            checks.append((ok, "search_engrams as an all-domain sweep (domains omitted)"))
        reach = search_expect.get("reaches_domain")
        if reach:
            ok = any(
                not _domains_of(c) or str(reach) in _domains_of(c)
                for c in searches
            )
            checks.append((ok, f"a search that can reach domain {reach!r} (sweep or scoped to it)"))
        want_status = search_expect.get("status")
        if want_status:
            ok = any(str(c.get("status", "") or "") == want_status for c in searches)
            checks.append((ok, f"search_engrams with status filter {want_status!r}"))
        if search_expect.get("forbid_status"):
            ok = bool(searches) and all(not c.get("status") for c in searches)
            checks.append((ok, "no status filter on any search (history question)"))
        want_filter_keys = search_expect.get("metadata_filters_keys")
        if want_filter_keys:
            def _has_keys(c: dict) -> bool:
                filters = c.get("metadata_filters") or {}
                return all(k in filters for k in want_filter_keys)
            ok = any(_has_keys(c) for c in searches)
            checks.append((ok, f"search_engrams with metadata_filters on {want_filter_keys}"))

    if expect.get("forbid_metadata_filters"):
        ok = bool(searches) and all(not c.get("metadata_filters") for c in searches)
        checks.append((ok, "no metadata_filters (window filters exclude unbounded engrams)"))

    if "manifest_read_ok" in expect and not expect.get("manifest_read_ok"):
        ok = not _is_manifest_read(tool_calls)
        checks.append((ok, "no MANIFEST read (search should suffice)"))

    if expect.get("require_manifest_read"):
        ok = _is_manifest_read(tool_calls)
        checks.append((ok, "the domain MANIFEST had to be read (structure question)"))

    anchor_prefix = expect.get("build_context_anchor")
    if anchor_prefix:
        contexts = _mcp_calls(tool_calls, "build_context")
        ok = any(
            str(c.get("anchor", "") or "").startswith(anchor_prefix)
            for c in contexts
        )
        checks.append((ok, f"build_context anchored at {anchor_prefix}"))

    if expect.get("forbid_write"):
        used = {
            c.get("name", "").removeprefix(MCP_PREFIX)
            for c in tool_calls
        } & WRITE_TOOLS
        checks.append((not used, f"no write tools (used: {sorted(used)})"))

    if not checks:
        return 0, 0.0, ["item defines no checks"]

    failed = [desc for ok, desc in checks if not ok]
    soft = (len(checks) - len(failed)) / len(checks)
    hard = int(not failed)
    return hard, soft, failed


def describe_expectations(expect: dict[str, Any]) -> str:
    """Render an item's expectations as reference text for the reflection
    stage, so the optimizer's analysts see what correct routing looked like."""
    lines: list[str] = []
    search_expect = expect.get("search") or {}
    if search_expect.get("domains") is not None:
        lines.append(
            "The correct move was a targeted search: search_engrams with "
            f"domains={search_expect['domains']}."
        )
    if search_expect.get("domains_omitted"):
        lines.append(
            "The correct move was an all-domain sweep: search_engrams with "
            "the domains argument omitted."
        )
    if search_expect.get("reaches_domain"):
        lines.append(
            "The knowledge lives in the "
            f"'{search_expect['reaches_domain']}' domain, which is not the "
            "obvious owner; an all-domain sweep or a scoped search that "
            "includes that domain was required, and pre-filtering to only "
            "the obvious domains fails."
        )
    if search_expect.get("status"):
        lines.append(
            f"A status filter of '{search_expect['status']}' was required "
            "on the search."
        )
    if search_expect.get("forbid_status"):
        lines.append(
            "No status filter should be used: the question needs engrams in "
            "every validity state, narrated by status."
        )
    if search_expect.get("metadata_filters_keys"):
        lines.append(
            "The question is about a bounded time window, so metadata_filters "
            f"on {search_expect['metadata_filters_keys']} were required."
        )
    if expect.get("forbid_metadata_filters"):
        lines.append(
            "This is a present-day question: metadata_filters on validity "
            "dates were wrong here because they exclude engrams that never "
            "set those fields; a status filter alone was correct."
        )
    if "manifest_read_ok" in expect and not expect.get("manifest_read_ok"):
        lines.append(
            "Reading a MANIFEST (read_engram on MANIFEST or browse_domain) "
            "was unnecessary here and counts as a failure."
        )
    if expect.get("manifest_read_ok"):
        lines.append("Reading the domain MANIFEST was acceptable for this task.")
    if expect.get("require_manifest_read"):
        lines.append(
            "The task asks about a domain's own structure, so reading its "
            "MANIFEST (read_engram or browse_domain) was required."
        )
    if expect.get("answer_all"):
        lines.append(
            f"The final answer had to mention all of {expect['answer_all']}."
        )
    if expect.get("build_context_anchor"):
        lines.append(
            "The task hands over an anchor, so build_context on "
            f"{expect['build_context_anchor']} was the expected way to gather "
            "the neighbourhood."
        )
    if expect.get("answer_any"):
        lines.append(
            f"The final answer had to mention one of {expect['answer_any']}."
        )
    if expect.get("forbid_write"):
        lines.append("No write tools were allowed; the task is read-only.")
    return "\n".join(lines)
