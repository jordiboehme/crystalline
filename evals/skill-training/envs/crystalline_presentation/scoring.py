"""Deterministic scoring for presentation tasks.

The presentation benchmark asks whether an agent that works the reflect
queue shows the shape of the content with the right element, marks a
status with a chip in the scheme colour, adds no decoration, and lets the
domain's PACT win over the skill's defaults. The
fixture workspace is ``beacon``: ``relay`` holds four engrams (a handshake
between three parties, an irreversible purge, three transport options with
verdicts and a one-line restart) and ``quiet`` holds the same engrams with
a PACT whose Presentation section says "No diagrams. Plain text and tables
only." Neither domain has a page yet.

Ground truth per item is an ``expect`` object whose ``checks`` list names
the item classes it is scored on. Each class is one function below and
returns ``(ok, description)`` pairs. Every "must not" class is paired with
a did-the-work check (a page covers the engram it is about), so writing
nothing never passes.

Evidence: the sandbox's reflection pages after the session (the
``reflection:`` frontmatter of a page records which section was built from
which engram, so "the section built from the handshake engram" is read from
the file, not from what the agent passed) and the verify snapshots before
and after.

Classes, each reading an optional ``expect[<class>]`` object with a
``domain`` (default ``relay``):

- ``diagram_for_protocol``: the section built from the handshake engram
  has a mermaid fence whose first word is ``sequenceDiagram``.
- ``callout_for_risk``: the section built from the purge engram has a
  ``> [!WARNING]`` or ``> [!CAUTION]`` callout that is not folded.
- ``table_for_comparison``: the section built from the transport engram
  has a GFM table with at least three body rows.
- ``status_chip``: the section built from the transport engram has a
  status chip with one of the six scheme colours, and a chip that says an
  option is recommended is green (one that says not recommended is red).
- ``no_decoration``: the restart how-to is covered, its section (when it
  is not shared with other engrams) has no mermaid fence and at most one
  callout, no page has two callouts in a row, and no page holds an element
  without content: an empty or type-only mermaid fence, a callout with no
  title and no body, an empty status chip or a table with no body rows.
  Nor does a page decorate: no mermaid diagram of one node and no edge,
  no callout whose body repeats the sentence right before it, and no more
  than three callouts on one page.
- ``pact_override``: a page covers the handshake, and no mermaid fence
  and no callout stands anywhere under the domain's reflections/ (the
  PACT said plain text and tables only).
- ``clean_dialect``: a page was written, the verify run after the session
  has no Q006 or Q007 issue (verify does not scan reflection pages, so the
  pages are also read for unknown callout kinds and status colours), every
  mermaid fence starts with a starter keyword, no engram gained a Q008 and
  PACT.md is byte-for-byte what it was (the must-not, paired with the
  did-the-work check that a page was written).

Plus ``answer_any`` / ``answer_all`` final-answer substrings.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

MCP_PREFIX = "mcp__crystalline__"
DEFAULT_DOMAIN = "relay"

PROTOCOL = "relay-handshake"
RISK = "tenant-purge"
COMPARISON = "transport-options"
HOWTO = "restart-the-relay"

STARTERS = {
    "graph", "flowchart", "sequenceDiagram", "stateDiagram-v2", "classDiagram",
    "erDiagram", "gantt", "pie", "timeline", "journey", "quadrantChart",
    "mindmap", "C4Context", "requirementDiagram", "architecture-beta",
    "xychart-beta", "radar-beta", "gitGraph",
}

Check = tuple[bool, str]
Issue = tuple[str, str, str]

CALLOUT_KINDS = {"NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION", "DETAILS"}
STATUS_COLOURS = {"grey", "blue", "green", "yellow", "red", "purple"}

_FENCE = re.compile(r"^\s*(`{3,}|~{3,})\s*([^\s`]*)")
_CALLOUT = re.compile(r"^\s*>\s*\[!([A-Za-z]+)\]([+-]?)")
_STATUS = re.compile(r":status\[[^\]]*\](?:\{([^}]*)\})?")
_CHIP = re.compile(r":status\[([^\]]*)\](?:\{([^}]*)\})?")
_ID_LINE = re.compile(r"^\s*<!--\s*r:([0-9a-fA-F]+)\s*-->\s*$")
_TABLE_DELIM = re.compile(r"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$")


@dataclass
class Evidence:
    tool_calls: list[dict]
    answer: str
    sandbox: Path
    issues_before: set[Issue] | None = None
    issues_after: set[Issue] | None = None
    pacts_before: dict[str, str] | None = None

    def pages(self, domain: str) -> dict[str, str]:
        """Every markdown file under the domain's reflections/, keyed by
        its path inside reflections/."""
        root = self.sandbox / "domains" / domain / "reflections"
        if not root.is_dir():
            return {}
        out: dict[str, str] = {}
        for p in sorted(root.rglob("*.md")):
            rel = p.relative_to(root)
            # `.reflected/` holds baseline copies and `*.threads.md` the
            # discussion threads: neither is a page the agent wrote.
            if any(part.startswith(".") for part in rel.parts):
                continue
            if p.name.endswith(".threads.md"):
                continue
            out[rel.as_posix()] = p.read_text(encoding="utf-8")
        return out


# ── markdown helpers ──────────────────────────────────────────────────────

@dataclass
class Fence:
    info: str
    lines: list[str]

    @property
    def first_word(self) -> str:
        for line in self.lines:
            if line.strip():
                return line.split()[0]
        return ""


def _walk(text: str):
    """Yield (line, fence) pairs: fence is the open Fence for a line inside
    a code fence (the fence lines themselves included), else None."""
    open_fence: Fence | None = None
    marker = ""
    for line in text.splitlines():
        m = _FENCE.match(line)
        if open_fence is None:
            if m:
                marker = m.group(1)
                open_fence = Fence(m.group(2).lower(), [])
                yield line, open_fence
                continue
            yield line, None
        else:
            if m and m.group(1)[0] == marker[0] and len(m.group(1)) >= len(marker) \
                    and not line.strip().strip("`~"):
                yield line, open_fence
                open_fence = None
                continue
            open_fence.lines.append(line)
            yield line, open_fence


def mermaid_fences(text: str) -> list[Fence]:
    seen: list[Fence] = []
    for _, fence in _walk(text):
        if fence is not None and fence.info == "mermaid" and fence not in seen:
            seen.append(fence)
    return seen


def callouts(text: str) -> list[tuple[str, str]]:
    """Every callout head outside code fences as (KIND, fold sign)."""
    out = []
    for line, fence in _walk(text):
        if fence is None and (m := _CALLOUT.match(line)):
            out.append((m.group(1).upper(), m.group(2)))
    return out


def consecutive_callouts(text: str) -> bool:
    """True when two callout heads follow each other with no other block
    between them: blocks are separated by blank lines, and a second head
    inside one quote counts as a second callout."""
    last_was_callout = False
    for lines in _blocks(text):
        heads = sum(1 for ln in lines if _CALLOUT.match(ln))
        is_callout = bool(_CALLOUT.match(lines[0]))
        if heads > 1 or (is_callout and last_was_callout):
            return True
        last_was_callout = is_callout
    return False


def _blocks(text: str) -> list[list[str]]:
    """The blocks of a text outside code fences, split at blank lines. A
    fenced block shows as a single ```` ``` ```` line."""
    block: list[str] = []
    blocks: list[list[str]] = []
    for line, fence in _walk(text):
        if fence is not None:
            block.append("```")
            continue
        if line.strip():
            block.append(line)
        elif block:
            blocks.append(block)
            block = []
    if block:
        blocks.append(block)
    return blocks


def chips(text: str) -> list[tuple[str, str | None]]:
    """Every status chip outside code fences as (label, colour or None)."""
    out = []
    for line, fence in _walk(text):
        if fence is None:
            for m in _CHIP.finditer(line):
                colour = m.group(2).strip().lower() if m.group(2) is not None else None
                out.append((m.group(1), colour))
    return out


def empty_elements(text: str) -> list[str]:
    """Every element that carries no content: a mermaid fence with no
    diagram under its type line, a callout with no title and no body, a
    status chip with no label, and a table with no body rows."""
    found: list[str] = []
    for f in mermaid_fences(text):
        if sum(1 for ln in f.lines if ln.strip()) < 2:
            found.append(f"an empty mermaid fence ({f.first_word or 'no type'})")
    for lines in _blocks(text):
        heads = [i for i, ln in enumerate(lines) if _CALLOUT.match(ln)]
        for n, i in enumerate(heads):
            m = _CALLOUT.match(lines[i])
            title = lines[i][m.end():].strip()
            stop = heads[n + 1] if n + 1 < len(heads) else len(lines)
            body = [ln.strip().lstrip(">").strip() for ln in lines[i + 1:stop]]
            if not title and not any(body):
                found.append(f"an empty {m.group(1).upper()} callout")
    for label, _ in chips(text):
        if not label.strip():
            found.append("an empty status chip")
    for rows in table_body_rows(text):
        if rows == 0:
            found.append("a table with no body rows")
    return found


# An edge in any diagram type: a flowchart or state arrow, a sequence
# message, an ER or class relation, a link with no arrow, a C4 relation.
_EDGE = re.compile(r"--|->|==|-\.|\.\.|~~~|\bRel\w*\(")

MAX_CALLOUTS = 3


def _plain(text: str) -> str:
    """Text reduced to its words, for comparing a callout with a sentence."""
    return " ".join(re.sub(r"[^\w\s]", " ", text.lower()).split())


def decorated_elements(text: str) -> list[str]:
    """Every element that has content but adds nothing: a mermaid diagram
    of one node and no edge, a callout that says again the sentence right
    before it, and more than ``MAX_CALLOUTS`` callouts on one page."""
    found: list[str] = []
    for f in mermaid_fences(text):
        body = [ln for ln in f.lines if ln.strip()][1:]
        if len(body) == 1 and not _EDGE.search(body[0]):
            found.append(f"a mermaid diagram with one node ({f.first_word})")
    previous: list[str] | None = None
    for lines in _blocks(text):
        heads = [i for i, ln in enumerate(lines) if _CALLOUT.match(ln)]
        if heads and heads[0] == 0 and previous and previous[0] != "```" \
                and not previous[0].startswith("#") and not _CALLOUT.match(previous[0]):
            m = _CALLOUT.match(lines[0])
            stop = heads[1] if len(heads) > 1 else len(lines)
            body = " ".join(ln.strip().lstrip(">").strip() for ln in lines[1:stop])
            said = body.strip() or lines[0][m.end():].strip()
            sentences = re.split(r"(?<=[.!?:])\s+", " ".join(ln.strip() for ln in previous))
            last = sentences[-1] if sentences else ""
            if _plain(said) and _plain(said) == _plain(last):
                found.append(f"a {m.group(1).upper()} callout that repeats the sentence before it")
        previous = lines
    count = len(callouts(text))
    if count > MAX_CALLOUTS:
        found.append(f"{count} callouts on one page (at most {MAX_CALLOUTS})")
    return found


def table_body_rows(text: str) -> list[int]:
    """The body-row count of every GFM table outside code fences."""
    counts = []
    lines = [ln for ln, fence in _walk(text) if fence is None]
    i = 0
    while i < len(lines) - 1:
        if "|" in lines[i] and _TABLE_DELIM.match(lines[i + 1]) and "|" in lines[i + 1]:
            j = i + 2
            while j < len(lines) and "|" in lines[j] and lines[j].strip():
                j += 1
            counts.append(j - i - 2)
            i = j
        else:
            i += 1
    return counts


def sections(page: str) -> list[tuple[str, str, str]]:
    """(id, heading text, body) for every ``##`` section of a page."""
    out: list[tuple[str, str, list[str]]] = []
    for line, fence in _walk(page):
        if fence is None and line.startswith("## "):
            out.append(("", line[3:].strip(), []))
            continue
        if out:
            sid, heading, body = out[-1]
            if not sid and not body and (m := _ID_LINE.match(line)):
                out[-1] = (m.group(1).lower(), heading, body)
                continue
            body.append(line)
    return [(sid, heading, "\n".join(body)) for sid, heading, body in out]


# ── coverage helpers ──────────────────────────────────────────────────────

def _engram_slug(ref: str) -> str:
    return str(ref).strip().rstrip("/").split("/")[-1].lower()


def page_sources(page: str) -> dict[str, set[str]]:
    """Section id to the engram slugs that section was built from, read from
    the page's own ``reflection:`` frontmatter, which is where Crystalline
    keeps what a ``write_reflection`` with ``sources`` recorded."""
    import yaml

    if not page.startswith("---\n"):
        return {}
    end = page.find("\n---\n", 4)
    if end < 0:
        return {}
    try:
        front = yaml.safe_load(page[4:end]) or {}
    except yaml.YAMLError:
        return {}
    records = front.get("reflection") if isinstance(front, dict) else None
    out: dict[str, set[str]] = {}
    for sid, record in (records or {}).items():
        refs = record.get("sources") if isinstance(record, dict) else None
        out[str(sid).lower()] = {
            _engram_slug(r.get("engram", "")) for r in refs or [] if isinstance(r, dict)
        }
    return out


def covered_sections(ev: Evidence, domain: str, engram: str) -> list[tuple[str, str, set[str]]]:
    """Sections of the domain's pages recorded as built from ``engram``:
    (page path, section body, every engram that section cites).

    The record is read from the files the session left behind, so only a
    write that Crystalline accepted counts, and a later rewrite that drops
    the sources takes the credit away with them.
    """
    found: list[tuple[str, str, set[str]]] = []
    for path, text in ev.pages(domain).items():
        recorded = page_sources(text)
        for sid, _heading, body in sections(text):
            cited = recorded.get(sid, set())
            if sid and engram in cited:
                found.append((path, body, cited))
    return found


def _no_section(engram: str) -> Check:
    return False, f"a section built from the '{engram}' engram has to exist (write_reflection with sources that Crystalline accepted)"


# ── scorers ───────────────────────────────────────────────────────────────

def score_diagram_for_protocol(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", DEFAULT_DOMAIN))
    secs = covered_sections(ev, domain, PROTOCOL)
    if not secs:
        return [_no_section(PROTOCOL)]
    return [(
        any(f.first_word == "sequenceDiagram" for _, body, _ in secs for f in mermaid_fences(body)),
        "parties exchanging ordered messages need a mermaid sequenceDiagram in the handshake section",
    )]


def score_callout_for_risk(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", DEFAULT_DOMAIN))
    secs = covered_sections(ev, domain, RISK)
    if not secs:
        return [_no_section(RISK)]
    good = any(
        kind in ("WARNING", "CAUTION") and fold != "-"
        for _, body, _ in secs
        for kind, fold in callouts(body)
    )
    return [(good, "the purge risk needs a > [!WARNING] or > [!CAUTION] callout that is not folded")]


def score_table_for_comparison(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", DEFAULT_DOMAIN))
    secs = covered_sections(ev, domain, COMPARISON)
    if not secs:
        return [_no_section(COMPARISON)]
    return [(
        any(n >= 3 for _, body, _ in secs for n in table_body_rows(body)),
        "the three transport options need a table with at least three body rows",
    )]


def score_status_chip(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", DEFAULT_DOMAIN))
    secs = covered_sections(ev, domain, COMPARISON)
    if not secs:
        return [_no_section(COMPARISON)]
    found = [c for _, body, _ in secs for c in chips(body)]
    coloured = [(label, colour) for label, colour in found if colour in STATUS_COLOURS]
    wrong = sorted({
        label.strip()
        for label, colour in found
        if "recommended" in label.lower()
        and colour != ("red" if re.search(r"\bnot\b", label.lower()) else "green")
    })
    return [
        (
            bool(coloured),
            "the transport verdicts need a status chip with a scheme colour (:status[...]{green} and so on)",
        ),
        (
            not wrong,
            f"a recommended option is green and a not recommended one red, not {wrong}",
        ),
    ]


def score_no_decoration(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", DEFAULT_DOMAIN))
    secs = covered_sections(ev, domain, HOWTO)
    checks: list[Check] = []
    if not secs:
        return [_no_section(HOWTO)]
    alone = [body for _, body, cited in secs if cited == {HOWTO}]
    checks.append((
        all(not mermaid_fences(b) for b in alone),
        "the one-line restart how-to needs no diagram",
    ))
    checks.append((
        all(len(callouts(b)) <= 1 for b in alone),
        "the one-line restart how-to takes at most one callout",
    ))
    pages = ev.pages(domain)
    checks.append((
        not any(consecutive_callouts(t) for t in pages.values()),
        "no page may hold two callouts in a row",
    ))
    empty = sorted({e for p, t in pages.items() if p != "PACT.md" for e in empty_elements(t)})
    checks.append((
        not empty,
        f"every element carries content, never decoration: found {empty}",
    ))
    extra = sorted({e for p, t in pages.items() if p != "PACT.md" for e in decorated_elements(t)})
    checks.append((
        not extra,
        f"every element adds something the text does not say: found {extra}",
    ))
    return checks


def score_pact_override(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", "quiet"))
    pages = ev.pages(domain)
    return [
        (
            bool(covered_sections(ev, domain, PROTOCOL)),
            "a page has to cover the handshake in plain text or a table",
        ),
        (
            not any(mermaid_fences(t) for t in pages.values()),
            "the PACT says no diagrams: no mermaid fence may stand under this domain's reflections",
        ),
        (
            not any(callouts(t) for p, t in pages.items() if p != "PACT.md"),
            "the PACT says plain text and tables only: no callout may stand under this domain's reflections",
        ),
    ]


def score_clean_dialect(ev: Evidence, spec: dict) -> list[Check]:
    domain = str(spec.get("domain", DEFAULT_DOMAIN))
    pages = ev.pages(domain)
    written = [p for p in pages if p != "PACT.md"]
    after = ev.issues_after or set()
    before = ev.issues_before or set()
    unknown = sorted({
        f"{f.first_word or '(empty)'}"
        for t in pages.values() for f in mermaid_fences(t)
        if f.first_word not in STARTERS
    })
    # `verify` scans engrams and the MANIFEST, not reflection pages, so the
    # kinds and colours a page uses are read from the page itself.
    bad_kinds = sorted({
        k for t in pages.values() for k, _ in callouts(t) if k not in CALLOUT_KINDS
    })
    bad_colours = sorted({
        m.group(1).strip().lower()
        for t in pages.values()
        for line, fence in _walk(t) if fence is None
        for m in _STATUS.finditer(line)
        if m.group(1) is not None and m.group(1).strip().lower() not in STATUS_COLOURS
    })
    q8 = sorted(path for path, rule, _ in after - before if rule == "Q008")
    pact_checks: list[Check] = []
    if ev.pacts_before is not None:
        now = {d: ev.pages(d).get("PACT.md") for d in ev.pacts_before}
        changed = sorted(d for d, text in ev.pacts_before.items() if now[d] != text)
        pact_checks.append((
            not changed,
            f"PACT.md is the person's agreement and stays as it is: changed in {changed}",
        ))
    return pact_checks + [
        (bool(written), "a reflection page has to be written"),
        (
            not [i for i in after if i[1] in ("Q006", "Q007")],
            "verify must report no unknown status colour (Q006) or callout kind (Q007)",
        ),
        (not bad_kinds, f"a callout kind must be one of the six, not {bad_kinds} (Q007)"),
        (not bad_colours, f"a status colour must be one of the six, not {bad_colours} (Q006)"),
        (
            not unknown,
            f"every mermaid fence must start with a starter keyword, not {unknown}",
        ),
        (not q8, f"no engram may gain an observation or relation inside a callout (Q008): {q8}"),
    ]


SCORERS: dict[str, Callable[[Evidence, dict], list[Check]]] = {
    "diagram_for_protocol": score_diagram_for_protocol,
    "callout_for_risk": score_callout_for_risk,
    "table_for_comparison": score_table_for_comparison,
    "status_chip": score_status_chip,
    "no_decoration": score_no_decoration,
    "pact_override": score_pact_override,
    "clean_dialect": score_clean_dialect,
}


def score_evidence(expect: dict, ev: Evidence) -> tuple[int, float, list[str]]:
    checks: list[Check] = []
    for name in expect.get("checks") or []:
        scorer = SCORERS.get(str(name))
        if scorer is None:
            checks.append((False, f"unknown check class '{name}'"))
            continue
        checks.extend(scorer(ev, expect.get(str(name)) or {}))
    lowered = (ev.answer or "").lower()
    if expect.get("answer_any"):
        checks.append((
            any(str(s).lower() in lowered for s in expect["answer_any"]),
            f"answer must mention one of {expect['answer_any']}",
        ))
    if expect.get("answer_all"):
        checks.append((
            all(str(s).lower() in lowered for s in expect["answer_all"]),
            f"answer must mention all of {expect['answer_all']}",
        ))
    if not checks:
        return 0, 0.0, ["item defines no checks"]
    failed = [desc for ok, desc in checks if not ok]
    soft = (len(checks) - len(failed)) / len(checks)
    return int(not failed), soft, failed


def score_item(
    expect: dict,
    tool_calls: list[dict],
    answer: str,
    sandbox: Path,
    prepared: dict,
    crystalline_bin: str,
) -> tuple[int, float, list[str]]:
    from envs import common

    after = common.snapshot(sandbox, crystalline_bin)
    ev = Evidence(
        tool_calls, answer, sandbox,
        pacts_before=prepared.get("pacts"),
        issues_before=set(prepared["verify_issues"]),
        issues_after=set(after["verify_issues"]),
    )
    return score_evidence(expect, ev)


_DESCRIPTIONS = {
    "diagram_for_protocol": (
        "Parties that exchange messages over time are a sequenceDiagram: the "
        "section built from the handshake engram carries a mermaid fence "
        "that starts with sequenceDiagram."
    ),
    "callout_for_risk": (
        "A step that cannot be undone gets a > [!WARNING] or > [!CAUTION] "
        "callout, never folded (no - after the bracket): the purge section "
        "carries one."
    ),
    "table_for_comparison": (
        "Options compared are a table: the transport section carries a "
        "table with one body row per option."
    ),
    "status_chip": (
        "Anything with a status gets a status chip in the scheme colour: the "
        "transport section marks each verdict with :status[...]{colour}, a "
        "recommended option green and a not recommended one red."
    ),
    "no_decoration": (
        "Every element carries content, never decoration. The one-line "
        "restart how-to may stay short (no diagram, at most one callout), "
        "no page holds two callouts in a row, and no page holds an empty "
        "diagram, callout, status chip or table, a diagram of one node, a "
        "callout that repeats the sentence before it or more than three "
        "callouts."
    ),
    "pact_override": (
        "The domain's PACT Presentation section says no diagrams, and the "
        "PACT wins over the skill's defaults: the handshake is covered in "
        "plain text or a table, and no mermaid fence and no callout is "
        "written (plain text and tables only)."
    ),
    "clean_dialect": (
        "Use only syntax Crystalline knows: a status colour from the six, a "
        "callout kind from the six, a mermaid fence that starts with a "
        "starter keyword, and no observation or relation inside a callout "
        "of an engram. Fix every markdown_findings entry a receipt reports. "
        "PACT.md is not rewritten."
    ),
}


def describe_expectations(expect: dict) -> str:
    """Render the item's ground truth for the reflect stage."""
    lines = [
        _DESCRIPTIONS[str(name)]
        for name in expect.get("checks") or []
        if str(name) in _DESCRIPTIONS
    ]
    if expect.get("answer_any"):
        lines.append(f"The answer had to mention one of {expect['answer_any']}.")
    if expect.get("answer_all"):
        lines.append(f"The answer had to mention all of {expect['answer_all']}.")
    return " ".join(lines)
