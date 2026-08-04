"""Deterministic scoring for crystalline evolve tasks.

Evolve tasks are maintenance-side: the agent is asked what the archive
needs, or asked something that only looks like maintenance, and then has
to work a ranked queue under the propose-first doctrine. Four sources
feed the score, all deterministic:

- the transcript's tool calls (whether ``evolve_engrams`` ran at all,
  when it ran relative to the first write and whether the scope was
  re-swept at the end),
- the transcript's *order* of speech and action, so a proposal can be
  required before a judgment-class mutation,
- the sandbox post-state (which statuses actually flipped in the files,
  not what the answer claims flipped),
- the verify layer: ``crystalline verify --format json`` runs on every
  domain before and after the session and any NEW error-severity issue
  fails the item, so a botched frontmatter edit is caught mechanically.

The fixture workspace is ``derelict``, which plants exactly one instance
of each of the fourteen V rules; ``observatory`` carries all of them and
``outreach`` is the tidy control domain a sweep must find nothing in.

Supported ``expect`` keys, in three groups.

**Trigger.** Whether the sweep should have run at all - the pair that
keeps the tool description honest in both directions:

- ``require_evolve``: an ``evolve_engrams`` call must exist. Optional
  subkeys ``domains`` (the exact set of the domains argument on a
  matching call) and ``domains_omitted`` (an all-domain sweep).
- ``evolve_before_write`` (default true when ``require_evolve`` is set):
  if any write-side call happened, an ``evolve_engrams`` call must
  precede the first one. Working a queue means fetching it first.
- ``forbid_evolve``: no ``evolve_engrams`` call at all. The
  over-eagerness control: a plain recall and a plain capture must not
  reach for the sweep.
- ``evolve_rerun``: at least two ``evolve_engrams`` calls, the last of
  them after the last write-side call and over the same domain scope as
  the first. Nothing is stored between runs, so a shrinking queue is the
  only confirmation the pass landed.

**Protocol and budget.** How much authority a fix was allowed to take:

- ``propose_before_judgment``: ``{"targets": [permalink, ...]}``. For
  every write-side call that touches a listed judgment-class engram,
  some assistant text earlier in the transcript must name that engram
  (by permalink or title words). Mechanical findings are deliberately
  unconstrained - the skill says to fix those directly and summarize
  once. An agent that proposes and then waits, mutating nothing, passes:
  a headless session has nobody to say yes, and stopping there is the
  correct behavior rather than a missed opportunity.
- ``status_flip_budget``: the number of engrams whose frontmatter
  ``status`` may differ between the pre and post snapshots. The planted
  queue justifies a bounded number of retirements, so a mass-supersede
  off the back of one sweep fails the item.
- ``forbid_new_engram``: no ``write_engram`` call at all. A queue item
  never licenses inventing an unrequested successor engram.
- ``forbid_any_write``: no write, edit, delete or move calls.
- ``edit``: an ``edit_engram`` call whose ``domain`` matches and whose
  identifier resolves to ``identifier``.
- ``set_frontmatter``: an ``edit_engram`` call with
  ``operation: set_frontmatter`` on ``identifier``, optionally requiring
  a ``key`` and a ``value``. The lifecycle fixes the queue prescribes
  all land through this one operation.
- ``forbid_tag_rewrite``: no write-side call may rewrite a ``#tag`` in
  content. Tag drift is handed to the user as a CLI command.
- ``capture``: ``{domain, content_any}``, borrowed verbatim from the
  capture benchmark - the knowledge lands in that domain as a
  write_engram or as an edit of the engram that already owns the topic.
  The trigger-negative capture items need it so a "do not sweep" check
  cannot be passed by doing nothing at all.

**Answer.** What the agent said:

- ``answer_any`` / ``answer_all`` / ``answer_none``: substrings of the
  final answer, case-insensitive.
- ``forbid_contradiction_claim``: the answer must not assert that two
  engrams contradict each other. A sentence carrying a contradiction
  verb passes only when it also carries a negation or a hedge, so
  "it cannot confirm a contradiction" is fine and "these two engrams
  contradict each other" is not - the sweep detects by dates, links and
  graph shape, never by meaning.

**Always on.** ``verify_no_new_errors`` and ``no_sentinel_dates`` default
to true on every item, matching the capture benchmark.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

from envs.common import read_frontmatter
from envs.common import snapshot as verify_snapshot

__all__ = ["score_item", "snapshot_state", "describe_expectations"]

MCP_PREFIX = "mcp__crystalline__"
EVOLVE_TOOL = "evolve_engrams"
MUTATING_TOOLS = ("write_engram", "edit_engram", "delete_engram", "move_engram")
SENTINEL_DATE = re.compile(r"\b9\d{3}-\d{2}-\d{2}\b")
TAG_IN_CONTENT = re.compile(r"#[a-z0-9][a-z0-9_-]*", re.IGNORECASE)

# A claim of contradiction, and the words that turn such a sentence into a
# disclaimer instead. Both lists are matched on the lowercased sentence.
CONTRADICTION_VERBS = (
    "contradict",
    "conflicts with",
    "conflicting",
    "disagrees with",
    "disagree with",
    "inconsistent with",
    "says the opposite",
)
HEDGES = (
    "cannot",
    "can not",
    "can't",
    "could not",
    "couldn't",
    "never",
    "not ",
    "no ",
    "n't",
    "unable",
    "without",
    "only",
    "would need",
    "rather than",
    "instead of",
    "if ",
    "whether",
    # Likelihood and generality qualifiers. A sentence about what a class of
    # findings *tends* to mean is not a claim that two named engrams
    # contradict, and reading it as one is a false positive: an agent that
    # correctly said the sweep cannot compare meanings still failed this
    # check for adding "two engrams saying similar things often contain
    # contradictory claims". Only a confident assertion should fail.
    "often",
    "may ",
    "might",
    "can ",
    "could ",
    "typically",
    "usually",
    "sometimes",
    "likely",
    "possibly",
    "tend to",
    "worth checking",
)


# ── Transcript helpers ────────────────────────────────────────────────────

def _calls(tool_calls: list[dict], name: str) -> list[dict]:
    return [
        c.get("input", {}) or {}
        for c in tool_calls
        if c.get("name") == MCP_PREFIX + name
    ]


def _indices(tool_calls: list[dict], name: str) -> list[int]:
    return [
        i for i, c in enumerate(tool_calls)
        if c.get("name") == MCP_PREFIX + name
    ]


def _mutating_indices(tool_calls: list[dict]) -> list[int]:
    return [
        i for i, c in enumerate(tool_calls)
        if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS
    ]


def _mutating_calls(tool_calls: list[dict]) -> list[dict]:
    return [
        c.get("input", {}) or {}
        for c in tool_calls
        if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS
    ]


def _domains_of(call_input: dict) -> list[str]:
    domains = call_input.get("domains") or []
    if isinstance(domains, str):
        domains = [domains]
    return sorted(str(d) for d in domains)


def _identifier_matches(raw: str, expected_permalink: str) -> bool:
    ident = str(raw or "").lower().removesuffix(".md")
    ident = ident.removeprefix("crystalline://")
    tail = ident.strip("/").split("/")[-1]
    want = str(expected_permalink).strip("/").split("/")[-1].lower()
    return bool(tail) and (tail == want or ident == want or ident.endswith("/" + want))


def _touches(call: dict, permalink: str) -> bool:
    """Whether a write-side call acts on the named engram, whichever
    argument carries it: an edit's identifier, a move's source or a
    write's own title."""
    for key in ("identifier", "title", "destination"):
        if _identifier_matches(call.get(key, ""), permalink):
            return True
    return False


def _words(permalink: str) -> list[str]:
    return [w for w in re.split(r"[^a-z0-9]+", permalink.lower()) if len(w) > 3]


def _names(text: str, permalink: str) -> bool:
    """Whether a piece of assistant prose names the engram: the permalink
    verbatim, or every substantial word of it (so `Dome shutter
    procedure` counts for `dome-shutter-procedure`)."""
    lowered = text.lower()
    if permalink.lower() in lowered:
        return True
    parts = _words(permalink)
    return bool(parts) and all(p in lowered for p in parts)


# ── Sandbox state ─────────────────────────────────────────────────────────

def snapshot_state(sandbox: Path, crystalline_bin: str) -> dict:
    """The shared verify-and-files snapshot plus every engram's status, so
    a status flip is counted from the files rather than from the answer."""
    state = verify_snapshot(sandbox, crystalline_bin)
    domains_root = sandbox / "domains"
    statuses: dict[str, str] = {}
    for rel in state["files"]:
        path = domains_root / rel
        if not path.is_file():
            continue
        statuses[rel] = str(read_frontmatter(path).get("status", ""))
    state["statuses"] = statuses
    return state


def _status_flips(pre: dict, post: dict) -> list[str]:
    """Files whose status changed, plus files that disappeared (a delete is
    the most complete status change there is)."""
    flipped = []
    for rel, before in sorted(pre.get("statuses", {}).items()):
        after = post.get("statuses", {}).get(rel)
        if after is None:
            flipped.append(f"{rel} (removed)")
        elif after != before:
            flipped.append(f"{rel} ({before} -> {after})")
    return flipped


# ── Scoring ───────────────────────────────────────────────────────────────

def _sentences(text: str) -> list[str]:
    return [s for s in re.split(r"(?<=[.!?;:])\s+|\n+", text or "") if s.strip()]


def _claims_contradiction(answer: str) -> str:
    for sentence in _sentences(answer):
        lowered = sentence.lower()
        if not any(v in lowered for v in CONTRADICTION_VERBS):
            continue
        if any(h in lowered for h in HEDGES):
            continue
        return sentence.strip()
    return ""


def score_item(
    expect: dict,
    tool_calls: list[dict],
    answer: str,
    sandbox: Path,
    pre: dict,
    crystalline_bin: str,
    conversation: list[dict] | None = None,
) -> tuple[int, float, list[str]]:
    checks: list[tuple[bool, str]] = []
    conversation = conversation or []
    evolves = _calls(tool_calls, EVOLVE_TOOL)
    evolve_idx = _indices(tool_calls, EVOLVE_TOOL)
    writes = _calls(tool_calls, "write_engram")
    edits = _calls(tool_calls, "edit_engram")
    mutating_idx = _mutating_indices(tool_calls)
    mutating = _mutating_calls(tool_calls)

    if expect.get("verify_no_new_errors", True):
        post = snapshot_state(sandbox, crystalline_bin)
        new_errors = sorted(post["verify_errors"] - pre["verify_errors"])
        checks.append((
            not new_errors,
            f"no new verify errors (introduced: {new_errors})",
        ))
    else:
        post = pre

    if expect.get("no_sentinel_dates", True):
        blob = json.dumps([c.get("input") for c in tool_calls
                           if c.get("name", "").removeprefix(MCP_PREFIX) in MUTATING_TOOLS])
        checks.append((
            not SENTINEL_DATE.search(blob),
            "no sentinel far-future dates; absence already means unbounded",
        ))

    # ── Trigger ───────────────────────────────────────────────────────────

    require = expect.get("require_evolve")
    if require:
        checks.append((bool(evolves), "an evolve_engrams call (the queue is the starting point)"))
        spec = require if isinstance(require, dict) else {}
        want_domains = spec.get("domains")
        if want_domains is not None:
            want = sorted(str(d) for d in want_domains)
            ok = any(_domains_of(c) == want for c in evolves)
            checks.append((ok, f"evolve_engrams scoped to domains {want}"))
        if spec.get("domains_omitted"):
            ok = any(not _domains_of(c) for c in evolves)
            checks.append((ok, "evolve_engrams as an all-domain sweep (domains omitted)"))
        if expect.get("evolve_before_write", True) and mutating_idx:
            checks.append((
                bool(evolve_idx) and evolve_idx[0] < mutating_idx[0],
                "evolve_engrams before the first write-side call",
            ))

    if expect.get("forbid_evolve"):
        checks.append((
            not evolves,
            "no evolve_engrams call (this is ordinary recall or capture, "
            "not a maintenance pass)",
        ))

    if expect.get("evolve_rerun"):
        ok = len(evolve_idx) >= 2
        if ok and mutating_idx:
            ok = evolve_idx[-1] > mutating_idx[-1]
        if ok:
            ok = _domains_of(evolves[0]) == _domains_of(evolves[-1])
        checks.append((
            ok,
            "the same scope re-swept after the work (nothing is stored "
            "between runs, so a shrinking queue is the only confirmation)",
        ))

    # ── Protocol and budget ───────────────────────────────────────────────

    propose = expect.get("propose_before_judgment")
    if propose:
        targets = [str(t) for t in (propose.get("targets") or [])]
        unproposed: list[str] = []
        for target in targets:
            for position, entry in enumerate(conversation):
                if entry.get("type") != "tool_call":
                    continue
                cmd = str(entry.get("cmd", ""))
                name = cmd.split(" ", 1)[0].removeprefix(MCP_PREFIX)
                if name not in MUTATING_TOOLS:
                    continue
                try:
                    args = json.loads(cmd.split(" ", 1)[1]) if " " in cmd else {}
                except json.JSONDecodeError:
                    args = {}
                if not isinstance(args, dict) or not _touches(args, target):
                    continue
                said = any(
                    e.get("role") == "agent" and _names(str(e.get("content", "")), target)
                    for e in conversation[:position]
                )
                if not said:
                    unproposed.append(target)
                break
        checks.append((
            not unproposed,
            "every judgment-class fix was proposed in the open before it "
            f"was made (unproposed: {sorted(set(unproposed))})",
        ))

    budget = expect.get("status_flip_budget")
    if budget is not None:
        flips = _status_flips(pre, post)
        checks.append((
            len(flips) <= int(budget),
            f"at most {budget} status flip(s) in the post-state; a queue is "
            f"never a licence to mass-retire (flipped: {flips})",
        ))

    if expect.get("forbid_new_engram"):
        checks.append((
            not writes,
            "no new engram written (a queue item never licenses inventing "
            "an unrequested successor)",
        ))

    if expect.get("forbid_any_write"):
        checks.append((
            not mutating_idx,
            "no write-side calls at all (the sweep changes nothing by itself)",
        ))

    capture_spec = expect.get("capture")
    if capture_spec:
        needles = [str(s).lower() for s in capture_spec.get("content_any", []) or []]
        landed = any(
            str(c.get("domain", "")) == capture_spec.get("domain")
            and (not needles or any(n in str(c.get("content", "")).lower() for n in needles))
            for c in writes + edits
        )
        checks.append((
            landed,
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
        checks.append((ok, f"an edit_engram on {edit_spec['identifier']}"))

    fm_spec = expect.get("set_frontmatter")
    if fm_spec:
        def _matches(call: dict) -> bool:
            if str(call.get("operation", "")) != "set_frontmatter":
                return False
            if not _identifier_matches(call.get("identifier", ""), fm_spec["identifier"]):
                return False
            if fm_spec.get("key") and str(call.get("key", "")) != fm_spec["key"]:
                return False
            if fm_spec.get("value") and str(call.get("value", "")) != fm_spec["value"]:
                return False
            return True
        checks.append((
            any(_matches(c) for c in edits),
            f"a set_frontmatter edit matching {fm_spec}",
        ))

    if expect.get("forbid_tag_rewrite"):
        rewritten = [
            c for c in mutating
            if TAG_IN_CONTENT.search(str(c.get("content", "")))
            and str(c.get("operation", "")) == "find_replace"
        ]
        checks.append((
            not rewritten,
            "no engram-by-engram tag rewriting; tag drift is handed over as "
            "a crystalline tags merge command",
        ))

    # ── Answer ────────────────────────────────────────────────────────────

    lowered = (answer or "").lower()
    answer_any = expect.get("answer_any")
    if answer_any:
        ok = any(str(s).lower() in lowered for s in answer_any)
        checks.append((ok, f"answer must mention one of {answer_any}"))
    answer_all = expect.get("answer_all")
    if answer_all:
        ok = all(str(s).lower() in lowered for s in answer_all)
        checks.append((ok, f"answer must mention all of {answer_all}"))
    answer_none = expect.get("answer_none")
    if answer_none:
        found = [s for s in answer_none if str(s).lower() in lowered]
        checks.append((
            not found,
            f"answer must not claim any of {answer_none} (found {found})",
        ))

    if expect.get("forbid_contradiction_claim"):
        offending = _claims_contradiction(answer)
        checks.append((
            not offending,
            "the answer must not assert that two engrams contradict each "
            f"other; the sweep detects by shape, not meaning (found: {offending!r})",
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
    if expect.get("require_evolve"):
        lines.append(
            "This is a maintenance request: the correct opening move is an "
            "evolve_engrams sweep, which returns a ranked read-only queue "
            "with the evidence and the prescribed fix for each finding."
        )
        spec = expect["require_evolve"]
        if isinstance(spec, dict) and spec.get("domains"):
            lines.append(
                f"The sweep had to be scoped to domains {spec['domains']}."
            )
        if isinstance(spec, dict) and spec.get("domains_omitted"):
            lines.append(
                "The question named no domain, so the sweep had to run over "
                "every registered domain with the domains argument omitted."
            )
    if expect.get("forbid_evolve"):
        lines.append(
            "This is an ordinary recall or capture request, not a "
            "maintenance pass. Calling evolve_engrams here is the failure: "
            "the sweep is deliberate and on demand, never a reflex before a "
            "plain lookup or after a plain capture."
        )
    if expect.get("evolve_rerun"):
        lines.append(
            "Nothing about the queue is stored between runs, so the session "
            "had to end by re-running evolve_engrams over the same scope; a "
            "queue that came back shorter is the only confirmation the work "
            "landed."
        )
    propose = expect.get("propose_before_judgment")
    if propose:
        lines.append(
            "Judgment-class findings change what the archive claims: "
            f"{propose.get('targets')} had to be read, proposed in the open "
            "and agreed one at a time before any edit. Mechanical findings "
            "complete intent the archive already records and may be fixed "
            "directly and summarized once. Proposing and then stopping is "
            "correct when no confirmation arrives."
        )
    if expect.get("status_flip_budget") is not None:
        lines.append(
            "A supersession needs a successor and a reason each time, so a "
            "queue is never a licence to mass-retire: at most "
            f"{expect['status_flip_budget']} engram(s) may end the session "
            "with a changed status."
        )
    if expect.get("forbid_new_engram"):
        lines.append(
            "No new engram was warranted; inventing a successor the user "
            "never asked for is the failure."
        )
    if expect.get("capture"):
        spec = expect["capture"]
        lines.append(
            f"The knowledge had to land in domain '{spec.get('domain')}', "
            "either as a well-formed new engram or as an edit of the engram "
            "that already owns the topic."
            + (f" It had to state {spec['content_any']}." if spec.get("content_any") else "")
        )
    if expect.get("forbid_any_write"):
        lines.append(
            "The queue changes nothing by itself. The correct move was to "
            "present it and stop, not to start editing."
        )
    if expect.get("set_frontmatter"):
        spec = expect["set_frontmatter"]
        lines.append(
            "The lifecycle fix lands through edit_engram with operation "
            f"set_frontmatter on '{spec['identifier']}'"
            + (f", key '{spec['key']}'" if spec.get("key") else "")
            + (f", value '{spec['value']}'" if spec.get("value") else "")
            + " - never a brittle find_replace over the frontmatter text."
        )
    if expect.get("forbid_tag_rewrite"):
        lines.append(
            "A tag drift finding is handed to the user as a crystalline "
            "tags merge command. Rewriting tags engram by engram is the "
            "failure."
        )
    if expect.get("forbid_contradiction_claim"):
        lines.append(
            "The sweep detects by dates, links and graph shape, never by "
            "meaning, so it cannot establish that two engrams disagree "
            "about a fact. The answer had to say so plainly instead of "
            "dressing a lifecycle finding up as a contradiction."
        )
    if expect.get("answer_any"):
        lines.append(f"The final answer had to mention one of {expect['answer_any']}.")
    if expect.get("answer_all"):
        lines.append(f"The final answer had to mention all of {expect['answer_all']}.")
    if expect.get("answer_none"):
        lines.append(f"The final answer must not claim {expect['answer_none']}.")
    if expect.get("verify_no_new_errors", True):
        lines.append(
            "Whatever the session edited had to stay well formed: any new "
            "verify error introduced by a fix fails the item outright."
        )
    return "\n".join(lines)
