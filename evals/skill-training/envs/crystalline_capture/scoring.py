"""Deterministic scoring for crystalline capture tasks.

Capture tasks are write-side: the agent is asked to store, refine or
deliberately not store knowledge. Scoring combines three sources, all
deterministic:

- the transcript's tool calls (which write-side tools ran, with what
  arguments and in what order),
- the sandbox post-state (what actually landed in the domain files),
- the verify layer: ``crystalline verify --format json`` runs on every
  domain before and after the session and any NEW error-severity issue
  fails the item, making the product's own quality bar the eval floor
  (Q001 thin content, E004 missing tags, T004 date order, T008 sentinel
  dates, L002 duplicate permalinks and the rest).

Supported ``expect`` keys:

- ``verify_no_new_errors`` (default true on every item).
- ``no_sentinel_dates`` (default true): no 9xxx-year date anywhere in a
  write-side call's content or metadata.
- ``search_before_write`` (default true): if any write-side call
  happened, a search_engrams call must precede the first one.
- ``write``: at least one write_engram call matching every given
  subkey: ``domain``, ``tags_required``, ``status`` ("current" also
  matches an omitted status), ``content_min_lines``, ``category``
  (observation category, e.g. "fact"), ``title_contains``,
  ``content_any`` (substrings, any-of, case-insensitive),
  ``metadata_has`` (keys) and ``metadata_lacks`` (keys; absent metadata
  counts as lacking). Use only when a NEW engram is genuinely required
  (bounded validity metadata exists only on writes).
- ``capture``: the knowledge must land in ``domain`` either as a
  write_engram matching the spec or as an edit_engram in that domain
  whose content carries one of ``content_any`` - the fair check for
  fresh knowledge that may have a plausible existing owner, since the
  skill itself prefers edit over create. Metadata and category subkeys
  apply only to the write path; an edit cannot set them and that is
  already the correct unbounded behavior.
- ``edit``: an edit_engram call whose ``domain`` matches and whose
  identifier resolves to ``identifier`` (bare permalink,
  domain-prefixed or crystalline:// URL all accepted).
- ``forbid_new_engram``: no write_engram call at all (edit-over-create).
- ``forbid_any_write``: no write/edit/delete/move calls (do-not-capture).
- ``supersede``: post-state, ``{old_identifier, domain}``: the old
  engram's file now carries status superseded or deprecated plus a
  superseded_by relation, and replacement knowledge exists (a new file
  in the domain, or an edit to another engram there).
- ``relation_to``: a newly created engram links ``[[Title]]``.
- ``answer_any`` / ``answer_all``: substrings of the final answer.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

from envs.common import run_cmd, sandbox_env

MCP_PREFIX = "mcp__crystalline__"
MUTATING_TOOLS = ("write_engram", "edit_engram", "delete_engram", "move_engram")
SENTINEL_DATE = re.compile(r"\b9\d{3}-\d{2}-\d{2}\b")


def _calls(tool_calls: list[dict], name: str) -> list[dict]:
    return [
        c.get("input", {}) or {}
        for c in tool_calls
        if c.get("name") == MCP_PREFIX + name
    ]


def _mutating_indices(tool_calls: list[dict]) -> list[int]:
    return [
        i for i, c in enumerate(tool_calls)
        if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS
    ]


# ── Sandbox state ─────────────────────────────────────────────────────────

def snapshot(sandbox: Path, crystalline_bin: str) -> dict:
    """Capture the pre-run domain state: verify errors and file listing."""
    domains_root = sandbox / "domains"
    errors: set[tuple[str, str]] = set()
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
            if str(issue.get("severity", "")).lower() != "error":
                continue
            rel = str(issue.get("path", "")).replace("\\", "/")
            rel = rel.split("/domains/", 1)[-1]
            errors.add((rel, str(issue.get("rule", ""))))
    files = {
        str(p.relative_to(domains_root))
        for p in domains_root.rglob("*.md")
    }
    return {"verify_errors": errors, "files": files}


def _frontmatter(path: Path) -> dict:
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


def _find_engram_file(sandbox: Path, domain: str, permalink: str) -> Path | None:
    domain_dir = sandbox / "domains" / domain
    slug = permalink.strip("/").split("/")[-1]
    for p in domain_dir.rglob("*.md"):
        if p.stem == slug:
            return p
    return None


def _identifier_matches(raw: str, expected_permalink: str) -> bool:
    ident = str(raw).lower().removesuffix(".md")
    ident = ident.removeprefix("crystalline://")
    tail = ident.strip("/").split("/")[-1]
    want = expected_permalink.lower()
    return tail == want or ident == want or ident.endswith("/" + want)


# ── Scoring ───────────────────────────────────────────────────────────────

def _write_matches(spec: dict, call: dict) -> bool:
    if spec.get("domain") and str(call.get("domain", "")) != spec["domain"]:
        return False
    if spec.get("tags_required"):
        tags = call.get("tags")
        if not isinstance(tags, list) or not tags:
            return False
    want_status = spec.get("status")
    if want_status:
        got = str(call.get("status") or "current")
        if got != want_status:
            return False
    min_lines = spec.get("content_min_lines")
    if min_lines:
        lines = [l for l in str(call.get("content", "")).splitlines() if l.strip()]
        if len(lines) < min_lines:
            return False
    category = spec.get("category")
    if category and f"- [{category}]" not in str(call.get("content", "")):
        return False
    title_contains = spec.get("title_contains")
    if title_contains and title_contains.lower() not in str(call.get("title", "")).lower():
        return False
    content_any = spec.get("content_any")
    if content_any:
        lowered = str(call.get("content", "")).lower()
        if not any(str(s).lower() in lowered for s in content_any):
            return False
    metadata = call.get("metadata")
    metadata = metadata if isinstance(metadata, dict) else {}
    for key in spec.get("metadata_has", []) or []:
        if key not in metadata:
            return False
    for key in spec.get("metadata_lacks", []) or []:
        if key in metadata:
            return False
    return True


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
    edits = _calls(tool_calls, "edit_engram")
    mutating = _mutating_indices(tool_calls)

    if expect.get("verify_no_new_errors", True):
        post = snapshot(sandbox, crystalline_bin)
        new_errors = sorted(post["verify_errors"] - pre["verify_errors"])
        checks.append((
            not new_errors,
            f"no new verify errors (introduced: {new_errors})",
        ))
        new_files = post["files"] - pre["files"]
    else:
        post = pre
        new_files = set()

    if expect.get("no_sentinel_dates", True):
        blob = json.dumps([c.get("input") for c in tool_calls
                           if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS])
        checks.append((
            not SENTINEL_DATE.search(blob),
            "no sentinel far-future dates; absence already means unbounded",
        ))

    if expect.get("search_before_write", True) and mutating:
        search_idx = [
            i for i, c in enumerate(tool_calls)
            if c.get("name") == MCP_PREFIX + "search_engrams"
        ]
        checks.append((
            bool(search_idx) and search_idx[0] < mutating[0],
            "search_engrams before the first write-side call (dedupe rule)",
        ))

    write_spec = expect.get("write")
    if write_spec:
        ok = any(_write_matches(write_spec, c) for c in writes)
        checks.append((ok, f"a write_engram matching {write_spec}"))

    capture_spec = expect.get("capture")
    if capture_spec:
        as_write = any(_write_matches(capture_spec, c) for c in writes)
        needles = [str(s).lower() for s in capture_spec.get("content_any", []) or []]
        as_edit = any(
            str(c.get("domain", "")) == capture_spec.get("domain")
            and (not needles or any(n in str(c.get("content", "")).lower() for n in needles))
            for c in edits
        )
        checks.append((
            as_write or as_edit,
            f"the knowledge lands in domain '{capture_spec.get('domain')}' "
            f"(new engram or edit of the owner) carrying {capture_spec.get('content_any')}",
        ))

    edit_spec = expect.get("edit")
    if edit_spec:
        ok = any(
            str(c.get("domain", "")) == edit_spec.get("domain", c.get("domain", ""))
            and _identifier_matches(c.get("identifier", ""), edit_spec["identifier"])
            for c in edits
        )
        checks.append((ok, f"an edit_engram on {edit_spec['identifier']} (edit over create)"))

    if expect.get("forbid_new_engram"):
        checks.append((
            not writes,
            "no new engram (the topic already has an owner; edit it instead)",
        ))

    if expect.get("forbid_any_write"):
        checks.append((
            not mutating,
            "no write-side calls at all (transient knowledge is not captured)",
        ))

    supersede = expect.get("supersede")
    if supersede:
        old = _find_engram_file(sandbox, supersede["domain"], supersede["old_identifier"])
        ok_old = False
        if old is not None:
            fm = _frontmatter(old)
            body = old.read_text(encoding="utf-8")
            ok_old = (
                str(fm.get("status", "")) in ("superseded", "deprecated")
                and "superseded_by" in body
            )
        checks.append((
            ok_old,
            f"old engram {supersede['old_identifier']} set to superseded with a superseded_by relation",
        ))
        replaced = bool(
            {f for f in new_files if f.startswith(supersede["domain"] + "/")}
        ) or any(
            str(c.get("domain", "")) == supersede["domain"]
            and not _identifier_matches(c.get("identifier", ""), supersede["old_identifier"])
            for c in edits
        )
        checks.append((
            replaced,
            "replacement knowledge written as its own current engram",
        ))

    relation_to = expect.get("relation_to")
    if relation_to:
        needle = f"[[{relation_to}]]".lower()
        slug = re.sub(r"[^a-z0-9]+", "-", relation_to.lower()).strip("-")
        ok = any(
            needle in (sandbox / "domains" / f).read_text(encoding="utf-8").lower()
            for f in new_files
        ) or any(
            needle in str(c.get("content", "")).lower()
            for c in writes + edits
        ) or any(
            # Capturing the knowledge inside the named engram itself
            # connects it intrinsically; a self-link would even be wrong.
            _identifier_matches(c.get("identifier", ""), slug)
            for c in edits
        )
        checks.append((ok, f"the captured knowledge links [[{relation_to}]] or lands inside that engram"))

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
    """Render the item's ground truth for the reflect stage."""
    lines: list[str] = []
    if expect.get("verify_no_new_errors", True):
        lines.append(
            "Whatever gets written must pass the verify layer: frontmatter "
            "tags present, at least 3 non-blank content lines, valid dates, "
            "no duplicate permalinks and resolving wikilinks."
        )
    if expect.get("no_sentinel_dates", True):
        lines.append(
            "Validity is unbounded by omission: never write a sentinel "
            "far-future date to mean forever."
        )
    if expect.get("search_before_write", True):
        lines.append(
            "The correct flow searches for existing knowledge before any "
            "write, so the write lands as an edit when an owner exists."
        )
    write_spec = expect.get("write")
    if write_spec:
        lines.append(
            f"A new engram was expected in domain '{write_spec.get('domain')}' "
            f"with frontmatter tags, substantial content "
            f"({write_spec.get('content_min_lines', 3)}+ non-blank lines)"
            + (f", observation category [{write_spec['category']}]" if write_spec.get("category") else "")
            + (f", metadata carrying {write_spec['metadata_has']}" if write_spec.get("metadata_has") else "")
            + (f", metadata omitting {write_spec['metadata_lacks']}" if write_spec.get("metadata_lacks") else "")
            + "."
        )
    capture_spec = expect.get("capture")
    if capture_spec:
        lines.append(
            f"The knowledge had to land in domain '{capture_spec.get('domain')}', "
            "either as a well-formed new engram or as an edit of the engram "
            "that already owns the topic; both are correct."
            + (f" It had to state {capture_spec['content_any']}." if capture_spec.get("content_any") else "")
            + (f" A new engram's metadata had to omit {capture_spec['metadata_lacks']}." if capture_spec.get("metadata_lacks") else "")
        )
    edit_spec = expect.get("edit")
    if edit_spec:
        lines.append(
            f"The topic already has an owner engram '{edit_spec['identifier']}' "
            f"in domain '{edit_spec.get('domain')}': the correct move was "
            "edit_engram on it, not a new engram."
        )
    if expect.get("forbid_new_engram"):
        lines.append("Creating a new engram for this topic is a dedupe failure.")
    if expect.get("forbid_any_write"):
        lines.append(
            "This information is transient session scratch; capturing it at "
            "all is the failure. Answer the actual question and store nothing."
        )
    supersede = expect.get("supersede")
    if supersede:
        lines.append(
            "New knowledge replaces old: write the replacement as its own "
            f"current engram, set '{supersede['old_identifier']}' to "
            "superseded and add a superseded_by relation on it."
        )
    if expect.get("relation_to"):
        lines.append(
            f"The new engram had to link [[{expect['relation_to']}]] so the "
            "knowledge graph stays connected."
        )
    if expect.get("answer_any"):
        lines.append(f"The final answer had to mention one of {expect['answer_any']}.")
    if expect.get("answer_all"):
        lines.append(f"The final answer had to mention all of {expect['answer_all']}.")
    return "\n".join(lines)
