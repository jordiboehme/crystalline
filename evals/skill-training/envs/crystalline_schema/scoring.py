"""Deterministic scoring for crystalline schema tasks.

Schema tasks exercise Picoschema authoring, infer_schema bootstrapping,
validate_engrams conformance checks and warn-to-strict promotion.
Scoring reads the transcript's tool calls, the sandbox post-state (the
schema engram that got written, the violation that got fixed) and the
verify layer before and after the session.

Supported ``expect`` keys (verify_no_new_errors and no_sentinel_dates
default on, as in the capture env):

- ``schema_engram``: post-state assertions on the schema engram whose
  frontmatter ``entity`` matches. Subkeys: ``domain``, ``entity``,
  ``requires`` (declared without the ``?`` modifier, under ``schema:``),
  ``optional`` (declared with ``?``), ``enums`` ({name: [values]}),
  ``relations`` ({name: "Target"} - Capitalized target, ``?`` on the
  name tolerated), ``fm_requires`` (required names under
  ``settings.frontmatter``), ``validation`` (settings.validation value;
  when the expectation is "warn", an absent settings.validation also
  passes since warn is the default).
- ``used_tool``: {name, domain?, type?} - an infer_schema or
  validate_engrams call with matching arguments appears.
- ``resolve_verify_issue``: {path_contains, rule} - the issue existed in
  the pre-run verify snapshot and is gone afterwards, any severity.
- ``write``: capture's write matcher (domain, tags_required,
  content_min_lines, engram_type and friends).
- ``forbid_any_write``: no write/edit/delete/move calls.
- ``answer_any`` / ``answer_all``: substrings of the final answer.
"""
from __future__ import annotations

import json
from pathlib import Path

from envs.common import read_frontmatter, snapshot
from envs.crystalline_capture.scoring import (
    MCP_PREFIX,
    MUTATING_TOOLS,
    SENTINEL_DATE,
    _calls,
    _mutating_indices,
    _write_matches,
)

__all__ = ["score_item", "snapshot", "describe_expectations"]


def _find_schema_file(sandbox: Path, domain: str, entity: str) -> Path | None:
    domain_dir = sandbox / "domains" / domain
    for p in sorted(domain_dir.rglob("*.md")):
        fm = read_frontmatter(p)
        if str(fm.get("type", "")) == "schema" and str(fm.get("entity", "")) == entity:
            return p
    return None


def _decl_name(key: str) -> tuple[str, bool]:
    """Split a Picoschema declaration key into (base name, optional)."""
    base = str(key)
    modifier = base.find("(")
    if modifier >= 0:
        base = base[:modifier]
    optional = base.endswith("?")
    return base.rstrip("?"), optional


def _decl_map(mapping: dict) -> dict[str, dict]:
    """Index declarations by base name with their modifiers and values."""
    out: dict[str, dict] = {}
    for key, value in (mapping or {}).items():
        name, optional = _decl_name(str(key))
        out[name] = {"key": str(key), "optional": optional, "value": value}
    return out


def _check_schema_engram(spec: dict, sandbox: Path) -> list[tuple[bool, str]]:
    checks: list[tuple[bool, str]] = []
    domain = spec.get("domain", "")
    entity = spec.get("entity", "")
    path = _find_schema_file(sandbox, domain, entity)
    checks.append((
        path is not None,
        f"a schema engram with entity '{entity}' exists in domain '{domain}'",
    ))
    if path is None:
        return checks

    fm = read_frontmatter(path)
    decls = _decl_map(fm.get("schema") if isinstance(fm.get("schema"), dict) else {})
    settings = fm.get("settings") if isinstance(fm.get("settings"), dict) else {}
    fm_decls = _decl_map(
        settings.get("frontmatter") if isinstance(settings.get("frontmatter"), dict) else {}
    )

    for name in spec.get("requires", []) or []:
        d = decls.get(name)
        checks.append((
            d is not None and not d["optional"],
            f"schema declares required field '{name}'",
        ))
    for name in spec.get("optional", []) or []:
        d = decls.get(name)
        checks.append((
            d is not None and d["optional"],
            f"schema declares optional field '{name}?'",
        ))
    for name, values in (spec.get("enums") or {}).items():
        d = decls.get(name)
        got = d["value"] if d and isinstance(d["value"], list) else None
        checks.append((
            d is not None and "(enum)" in d["key"] and sorted(map(str, got or [])) == sorted(map(str, values)),
            f"schema declares '{name}(enum)' with values {values}",
        ))
    for name, target in (spec.get("relations") or {}).items():
        d = decls.get(name)        # a relation is just a Capitalized value
        value = str(d["value"]) if d else ""
        value = value.split(",")[0].strip()
        checks.append((
            d is not None and value == target,
            f"schema declares relation '{name}' targeting {target}",
        ))
    for name in spec.get("fm_requires", []) or []:
        d = fm_decls.get(name)
        checks.append((
            d is not None and not d["optional"],
            f"schema requires frontmatter field '{name}'",
        ))
    want_validation = spec.get("validation")
    if want_validation:
        got = str(settings.get("validation") or "warn")
        checks.append((
            got == want_validation,
            f"settings.validation is '{want_validation}'",
        ))
    return checks


def score_item(
    expect: dict,
    tool_calls: list[dict],
    answer: str,
    sandbox: Path,
    pre: dict,
    crystalline_bin: str,
) -> tuple[int, float, list[str]]:
    checks: list[tuple[bool, str]] = []
    writes = _calls(tool_calls, "write_engram")
    mutating = _mutating_indices(tool_calls)
    post = None

    def _post() -> dict:
        nonlocal post
        if post is None:
            post = snapshot(sandbox, crystalline_bin)
        return post

    if expect.get("verify_no_new_errors", True):
        new_errors = sorted(_post()["verify_errors"] - pre["verify_errors"])
        checks.append((
            not new_errors,
            f"no new verify errors (introduced: {new_errors})",
        ))

    if expect.get("no_sentinel_dates", True):
        blob = json.dumps([
            c.get("input") for c in tool_calls
            if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS
        ])
        checks.append((
            not SENTINEL_DATE.search(blob),
            "no sentinel far-future dates",
        ))

    schema_spec = expect.get("schema_engram")
    if schema_spec:
        checks.extend(_check_schema_engram(schema_spec, sandbox))

    tool_spec = expect.get("used_tool")
    if tool_spec:
        calls = _calls(tool_calls, tool_spec["name"])
        def _matches(c: dict) -> bool:
            if tool_spec.get("domain") and str(c.get("domain", "")) != tool_spec["domain"]:
                return False
            if tool_spec.get("type") and str(c.get("type", "")) != tool_spec["type"]:
                return False
            return True
        checks.append((
            any(_matches(c) for c in calls),
            f"a {tool_spec['name']} call matching {tool_spec}",
        ))

    resolve = expect.get("resolve_verify_issue")
    if resolve:
        def _hit(issues: set) -> bool:
            return any(
                resolve["path_contains"] in path and rule == resolve["rule"]
                for (path, rule, _severity) in issues
            )
        was_there = _hit(pre["verify_issues"])
        gone = not _hit(_post()["verify_issues"])
        checks.append((
            was_there and gone,
            f"{resolve['rule']} on {resolve['path_contains']} resolved without regressions",
        ))

    write_spec = expect.get("write")
    if write_spec:
        ok = any(_write_matches(write_spec, c) for c in writes)
        checks.append((ok, f"a write_engram matching {write_spec}"))

    if expect.get("forbid_any_write"):
        checks.append((
            not mutating,
            "no write-side calls at all (a report, not a change)",
        ))

    lowered = (answer or "").lower()
    answer_any = expect.get("answer_any")
    if answer_any:
        ok = any(str(s).lower() in lowered for s in answer_any)
        checks.append((ok, f"answer must mention one of {answer_any}"))
    answer_all = expect.get("answer_all")
    if answer_all:
        ok = all(str(s).lower() in lowered for s in answer_all)
        checks.append((ok, f"answer must mention all of {answer_all}"))

    if not checks:
        return 0, 0.0, ["item defines no checks"]

    failed = [desc for ok, desc in checks if not ok]
    soft = (len(checks) - len(failed)) / len(checks)
    hard = int(not failed)
    return hard, soft, failed


def describe_expectations(expect: dict) -> str:
    lines: list[str] = []
    if expect.get("verify_no_new_errors", True):
        lines.append(
            "Nothing written may introduce a verify error; a schema engram "
            "must be well formed (entity present, declarations parseable, "
            "validation one of warn, strict or off)."
        )
    schema_spec = expect.get("schema_engram")
    if schema_spec:
        parts = [
            f"A schema engram (type schema) with entity '{schema_spec.get('entity')}' "
            f"was expected in domain '{schema_spec.get('domain')}'"
        ]
        if schema_spec.get("requires"):
            parts.append(f"declaring required fields {schema_spec['requires']}")
        if schema_spec.get("optional"):
            parts.append(f"optional fields {schema_spec['optional']} (trailing ?)")
        if schema_spec.get("enums"):
            parts.append(f"enums {schema_spec['enums']}")
        if schema_spec.get("relations"):
            parts.append(
                f"relations {schema_spec['relations']} (Capitalized type name = relation)"
            )
        if schema_spec.get("fm_requires"):
            parts.append(
                f"required settings.frontmatter fields {schema_spec['fm_requires']}"
            )
        if schema_spec.get("validation"):
            parts.append(f"settings.validation '{schema_spec['validation']}'")
        lines.append("; ".join(parts) + ". The whole shape lives in frontmatter.")
    tool_spec = expect.get("used_tool")
    if tool_spec:
        lines.append(
            f"The task required actually calling {tool_spec['name']} "
            f"with {json.dumps({k: v for k, v in tool_spec.items() if k != 'name'})} "
            "rather than answering from memory."
        )
    resolve = expect.get("resolve_verify_issue")
    if resolve:
        lines.append(
            f"The conformance issue {resolve['rule']} on "
            f"{resolve['path_contains']} had to be fixed in place by editing "
            "that engram, and nothing else may break."
        )
    if expect.get("write"):
        lines.append(
            f"A conforming new engram matching {expect['write']} was expected; "
            "under a strict schema a nonconforming write is a verify error."
        )
    if expect.get("forbid_any_write"):
        lines.append("This is a report task; no write-side tool may run.")
    if expect.get("answer_any"):
        lines.append(f"The final answer had to mention one of {expect['answer_any']}.")
    if expect.get("answer_all"):
        lines.append(f"The final answer had to mention all of {expect['answer_all']}.")
    return "\n".join(lines)
