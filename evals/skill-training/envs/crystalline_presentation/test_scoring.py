"""Unit tests for the presentation scorers: one passing and one failing
hand-written page per class.

    uv run --with pytest pytest envs/crystalline_presentation
"""
from __future__ import annotations

import json
from pathlib import Path

from envs.crystalline_presentation import scoring as s

SEQ = "```mermaid\nsequenceDiagram\n    Client->>Relay: hello\n    Relay-->>Client: ready\n```\n"
WARN = "> [!WARNING] A purge cannot be undone\n> Export the tenant first.\n"
TABLE = (
    "| Transport | Verdict |\n| --- | --- |\n| WebSocket | Recommended |\n"
    "| Long polling | Fallback only |\n| gRPC | Data centre only |\n"
)


def page(*sections: tuple[str, str, str], cites: dict[str, list[str]] | None = None) -> str:
    """A page from (id, heading, body) sections, with lantern's id lines.
    ``cites`` maps a section id to the engrams its frontmatter record names,
    the way Crystalline stores a write_reflection with sources."""
    front = ""
    if cites:
        front = "---\nreflection:\n"
        for sid, engrams in cites.items():
            front += f"  {sid}:\n    sources:\n"
            for e in engrams:
                front += f"    - engram: {e}\n      checksum: abc\n"
        front += "---\n\n"
    out = front + "# Relay\n\n"
    for sid, heading, body in sections:
        out += f"## {heading}\n<!-- r:{sid} -->\n\n{body}\n"
    return out


def put(tmp: Path, text: str, domain: str = "relay", rel: str = "guides/relay.md") -> None:
    path = tmp / "domains" / domain / "reflections" / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def ev(tmp: Path, **kw) -> s.Evidence:
    return s.Evidence([], "", tmp, **kw)


def ok(checks) -> bool:
    return all(good for good, _ in checks)


# ── markdown helpers ──────────────────────────────────────────────────────

def test_fences_callouts_and_tables_ignore_code_blocks():
    text = "```text\n> [!WARNING]\n```\n" + WARN + TABLE
    assert s.callouts(text) == [("WARNING", "")]
    assert s.table_body_rows(text) == [3]
    assert [f.first_word for f in s.mermaid_fences(SEQ)] == ["sequenceDiagram"]


def test_consecutive_callouts_need_no_block_between():
    assert s.consecutive_callouts(WARN + "\n" + "> [!NOTE]\n> more\n")
    assert not s.consecutive_callouts(WARN + "\nA sentence.\n\n> [!NOTE]\n> more\n")


# ── diagram_for_protocol ──────────────────────────────────────────────────

def test_diagram_for_protocol(tmp_path):
    put(tmp_path, page(("11111111", "Joining", "A client joins.\n\n" + SEQ), cites={"11111111": ["relay-handshake"]}))
    assert ok(s.score_diagram_for_protocol(ev(tmp_path), {}))


def test_diagram_for_protocol_fails_on_prose_wrong_type_or_other_topic(tmp_path):
    cites = {"11111111": ["relay-handshake"]}
    put(tmp_path, page(("11111111", "Joining", "A client joins."), cites=cites))
    assert not ok(s.score_diagram_for_protocol(ev(tmp_path), {}))
    put(tmp_path, page(("11111111", "Joining", SEQ.replace("sequenceDiagram", "flowchart TD")), cites=cites))
    assert not ok(s.score_diagram_for_protocol(ev(tmp_path), {}))
    # a diagram in a section that cites another engram, or none, does not count
    put(tmp_path, page(("11111111", "Joining", SEQ), cites={"11111111": ["tenant-purge"]}))
    assert not ok(s.score_diagram_for_protocol(ev(tmp_path), {}))
    put(tmp_path, page(("11111111", "Joining", SEQ)))
    assert not ok(s.score_diagram_for_protocol(ev(tmp_path), {}))


# ── callout_for_risk ──────────────────────────────────────────────────────

def test_callout_for_risk(tmp_path):
    cites = {"22222222": ["crystalline://relay/tenant-purge"]}
    put(tmp_path, page(("22222222", "Purge", WARN), cites=cites))
    assert ok(s.score_callout_for_risk(ev(tmp_path), {}))
    put(tmp_path, page(("22222222", "Purge", WARN.replace("WARNING]", "CAUTION]")), cites=cites))
    assert ok(s.score_callout_for_risk(ev(tmp_path), {}))
    put(tmp_path, page(("22222222", "Purge", WARN.replace("WARNING]", "WARNING]+")), cites=cites))
    assert ok(s.score_callout_for_risk(ev(tmp_path), {}))


def test_callout_for_risk_fails_when_folded_or_missing(tmp_path):
    cites = {"22222222": ["tenant-purge"]}
    put(tmp_path, page(("22222222", "Purge", WARN.replace("WARNING]", "WARNING]-")), cites=cites))
    assert not ok(s.score_callout_for_risk(ev(tmp_path), {}))
    put(tmp_path, page(("22222222", "Purge", "- A purge cannot be undone.\n"), cites=cites))
    assert not ok(s.score_callout_for_risk(ev(tmp_path), {}))
    put(tmp_path, page(("22222222", "Purge", "> [!NOTE]\n> Careful.\n"), cites=cites))
    assert not ok(s.score_callout_for_risk(ev(tmp_path), {}))


# ── table_for_comparison ──────────────────────────────────────────────────

def test_table_for_comparison(tmp_path):
    put(tmp_path, page(("33333333", "Transports", TABLE), cites={"33333333": ["transport-options"]}))
    assert ok(s.score_table_for_comparison(ev(tmp_path), {}))


def test_table_for_comparison_fails_on_list_or_short_table(tmp_path):
    cites = {"33333333": ["transport-options"]}
    put(tmp_path, page(("33333333", "Transports", "- WebSocket\n- Long polling\n- gRPC\n"), cites=cites))
    assert not ok(s.score_table_for_comparison(ev(tmp_path), {}))
    short = "| Transport | Verdict |\n| --- | --- |\n| WebSocket | Yes |\n"
    put(tmp_path, page(("33333333", "Transports", short), cites=cites))
    assert not ok(s.score_table_for_comparison(ev(tmp_path), {}))


# ── status_chip ───────────────────────────────────────────────────────────

CHIPS = (
    "| Transport | Verdict |\n| --- | --- |\n"
    "| WebSocket | :status[Recommended]{green} |\n"
    "| Long polling | :status[Fallback only]{yellow} |\n"
    "| gRPC | :status[Data centre only]{Yellow} |\n"
)


def test_status_chip(tmp_path):
    put(tmp_path, page(("33333333", "Transports", CHIPS), cites={"33333333": ["transport-options"]}))
    assert ok(s.score_status_chip(ev(tmp_path), {}))
    red = CHIPS.replace("Data centre only]{Yellow}", "Not recommended]{red}")
    put(tmp_path, page(("33333333", "Transports", red), cites={"33333333": ["transport-options"]}))
    assert ok(s.score_status_chip(ev(tmp_path), {}))


def test_status_chip_fails_on_words_only_wrong_colour_or_other_section(tmp_path):
    cites = {"33333333": ["transport-options"]}
    put(tmp_path, page(("33333333", "Transports", TABLE), cites=cites))
    assert not ok(s.score_status_chip(ev(tmp_path), {}))  # verdicts as plain words
    put(tmp_path, page(("33333333", "Transports", CHIPS.replace("{green}", "{blue}")), cites=cites))
    assert not ok(s.score_status_chip(ev(tmp_path), {}))  # recommended is green
    grey = CHIPS.replace("{green}", "").replace("{yellow}", "{pink}").replace("{Yellow}", "")
    put(tmp_path, page(("33333333", "Transports", grey), cites=cites))
    assert not ok(s.score_status_chip(ev(tmp_path), {}))  # no scheme colour at all
    put(tmp_path, page(("33333333", "Transports", CHIPS), cites={"33333333": ["tenant-purge"]}))
    assert not ok(s.score_status_chip(ev(tmp_path), {}))  # not the transport section


def test_status_chip_reads_not_with_a_hyphen(tmp_path):
    cites = {"33333333": ["transport-options"]}
    hyphen = CHIPS.replace("Data centre only]{Yellow}", "Not-recommended]{red}")
    put(tmp_path, page(("33333333", "Transports", hyphen), cites=cites))
    assert ok(s.score_status_chip(ev(tmp_path), {}))
    put(tmp_path, page(("33333333", "Transports", hyphen.replace("{red}", "{green}")), cites=cites))
    assert not ok(s.score_status_chip(ev(tmp_path), {}))  # not recommended is red


# ── no_decoration ─────────────────────────────────────────────────────────

COVER = {"44444444": ["restart-the-relay"], "22222222": ["tenant-purge"]}


def test_no_decoration(tmp_path):
    put(tmp_path, page(
        ("44444444", "Restart", "Run `relay restart`."),
        ("22222222", "Purge", WARN),
        cites=COVER,
    ))
    assert ok(s.score_no_decoration(ev(tmp_path), {}))


def test_no_decoration_fails_on_decorated_howto_or_stacked_callouts(tmp_path):
    put(tmp_path, page(("44444444", "Restart", "Run it.\n\n" + SEQ), cites=COVER))
    assert not ok(s.score_no_decoration(ev(tmp_path), {}))
    stacked = "Run it.\n\n> [!NOTE]\n> a\n\n> [!TIP]\n> b\n"
    put(tmp_path, page(("44444444", "Restart", stacked), cites=COVER))
    assert not ok(s.score_no_decoration(ev(tmp_path), {}))
    put(tmp_path, page(
        ("44444444", "Restart", "Run it."),
        ("22222222", "Purge", WARN + "\nA sentence.\n\n" + WARN + "\nAnother.\n\n" + WARN),
        cites=COVER,
    ))
    assert ok(s.score_no_decoration(ev(tmp_path), {}))  # callouts apart from each other are fine
    put(tmp_path, page(("44444444", "Restart", "Run it.")))
    assert not ok(s.score_no_decoration(ev(tmp_path), {}))  # the how-to is not covered


def test_empty_elements_names_each_contentless_element():
    assert s.empty_elements(SEQ + WARN + TABLE + "State :status[Done]{green}.\n") == []
    assert s.empty_elements("```mermaid\nflowchart TD\n```\n") == ["an empty mermaid fence (flowchart)"]
    assert s.empty_elements("```mermaid\n```\n") == ["an empty mermaid fence (no type)"]
    assert s.empty_elements("> [!NOTE]\n") == ["an empty NOTE callout"]
    assert s.empty_elements("> [!NOTE]\n>\n") == ["an empty NOTE callout"]
    assert s.empty_elements("> [!NOTE] Reads stay on the old cluster\n") == []
    assert s.empty_elements("State :status[]{green} and :status[ ].\n") == ["an empty status chip"] * 2
    assert s.empty_elements("| A | B |\n| --- | --- |\n") == ["a table with no body rows"]
    assert s.empty_elements("```text\n> [!NOTE]\n:status[]\n```\n") == []


def test_no_decoration_fails_on_a_contentless_element(tmp_path):
    for empty in (
        "```mermaid\nsequenceDiagram\n```\n",
        "> [!TIP]\n",
        "Status :status[]{grey}.\n",
        "| Step | Who |\n| --- | --- |\n",
    ):
        put(tmp_path, page(
            ("44444444", "Restart", "Run it."),
            ("22222222", "Purge", WARN + "\nA sentence.\n\n" + empty),
            cites=COVER,
        ))
        assert not ok(s.score_no_decoration(ev(tmp_path), {})), empty


def test_decorated_elements_names_each_element_that_adds_nothing():
    assert s.decorated_elements(SEQ + "\nA sentence.\n\n" + WARN + TABLE) == []
    one = "```mermaid\nflowchart TD\n    A[Restart the relay]\n```\n"
    assert s.decorated_elements(one) == ["a mermaid diagram with one node (flowchart)"]
    assert s.decorated_elements("```mermaid\nflowchart TD\n    A --> B\n```\n") == []
    assert s.decorated_elements("```mermaid\nflowchart TD\n    A[One]\n    B[Two]\n```\n") == []
    assert s.decorated_elements("```mermaid\npie\n    \"All\" : 100\n```\n") == [
        "a mermaid diagram with one node (pie)"
    ]
    echo = "Run it once. A purge cannot be undone.\n\n> [!WARNING]\n> A purge cannot be undone!\n"
    assert s.decorated_elements(echo) == ["a WARNING callout that repeats the sentence before it"]
    titled = "A purge cannot be undone.\n\n> [!WARNING] A purge cannot be undone\n"
    assert s.decorated_elements(titled) == ["a WARNING callout that repeats the sentence before it"]
    assert s.decorated_elements("A purge cannot be undone.\n\n" + WARN) == []
    assert s.decorated_elements("## A purge cannot be undone\n\n> [!NOTE]\n> A purge cannot be undone.\n") == []
    four = "\n\nText.\n\n".join([WARN] * 4)
    assert s.decorated_elements(four) == ["4 callouts on one page (at most 3)"]
    assert s.decorated_elements("\n\nText.\n\n".join([WARN] * 3)) == []
    assert s.decorated_elements("```text\nflowchart TD\n    A\n```\n") == []


def test_no_decoration_fails_on_an_element_that_adds_nothing(tmp_path):
    for extra in (
        "```mermaid\nflowchart LR\n    A[Purge]\n```\n",
        "Export the tenant first.\n\n> [!TIP]\n> Export the tenant first.\n",
        "\n\nText.\n\n".join([WARN] * 3),
    ):
        put(tmp_path, page(
            ("44444444", "Restart", "Run it."),
            ("22222222", "Purge", WARN + "\nA sentence.\n\n" + extra),
            cites=COVER,
        ))
        assert not ok(s.score_no_decoration(ev(tmp_path), {})), extra


# ── pact_override ─────────────────────────────────────────────────────────

def test_pact_override(tmp_path):
    table = "| Step | Who |\n| --- | --- |\n| hello | client |\n"
    put(tmp_path, page(("11111111", "Joining", table), cites={"11111111": ["relay-handshake"]}), domain="quiet")
    assert ok(s.score_pact_override(ev(tmp_path), {}))


def test_pact_override_fails_on_any_diagram_in_quiet(tmp_path):
    cites = {"11111111": ["relay-handshake"]}
    put(tmp_path, page(("11111111", "Joining", SEQ), cites=cites), domain="quiet")
    assert not ok(s.score_pact_override(ev(tmp_path), {}))
    # a diagram in the relay domain is not the quiet domain's business
    put(tmp_path, page(("11111111", "Joining", "Plain."), cites=cites), domain="quiet")
    put(tmp_path, page(("11111111", "Joining", SEQ), cites=cites), domain="relay")
    assert ok(s.score_pact_override(ev(tmp_path), {}))
    (tmp_path / "domains/quiet/reflections/guides/relay.md").unlink()
    assert not ok(s.score_pact_override(ev(tmp_path), {}))  # nothing written


def test_pact_override_fails_on_a_callout_in_quiet(tmp_path):
    cites = {"11111111": ["relay-handshake"]}
    caution = "Plain.\n\n> [!CAUTION]\n> The purge cannot be undone.\n"
    put(tmp_path, page(("11111111", "Joining", caution), cites=cites), domain="quiet")
    assert not ok(s.score_pact_override(ev(tmp_path), {}))


# ── clean_dialect ─────────────────────────────────────────────────────────

CLEAN_ISSUES = {("relay/tenant-purge.md", "T006", "warning")}


def test_clean_dialect(tmp_path):
    put(tmp_path, page(("11111111", "Joining", SEQ + "\n```mermaid\ngitGraph\n  commit\n```\n")))
    e = ev(tmp_path, issues_before=CLEAN_ISSUES, issues_after=CLEAN_ISSUES)
    assert ok(s.score_clean_dialect(e, {}))


def test_clean_dialect_fails_on_each_problem(tmp_path):
    put(tmp_path, page(("11111111", "Joining", SEQ)))
    for extra in (
        {("relay/reflections/guides/relay.md", "Q007", "warning")},
        {("relay/reflections/guides/relay.md", "Q006", "warning")},
        {("relay/tenant-purge.md", "Q008", "warning")},
    ):
        e = ev(tmp_path, issues_before=CLEAN_ISSUES, issues_after=CLEAN_ISSUES | extra)
        assert not ok(s.score_clean_dialect(e, {})), extra
    put(tmp_path, page(("11111111", "Joining", "```mermaid\nflow TD\n  A-->B\n```\n")))
    e = ev(tmp_path, issues_before=set(), issues_after=set())
    assert not ok(s.score_clean_dialect(e, {}))


def test_clean_dialect_reads_pages_for_unknown_kinds_and_colours(tmp_path):
    e = ev(tmp_path, issues_before=set(), issues_after=set())
    put(tmp_path, page(("11111111", "Joining", "> [!DANGER] x\n> y\n")))
    assert not ok(s.score_clean_dialect(e, {}))
    put(tmp_path, page(("11111111", "Joining", "State :status[Late]{pink}.\n")))
    assert not ok(s.score_clean_dialect(e, {}))
    put(tmp_path, page(("11111111", "Joining", "State :status[Late]{Red} and :status[Ok] at 10:30.\n")))
    assert ok(s.score_clean_dialect(e, {}))


def test_clean_dialect_does_not_blame_a_q008_that_was_there_before(tmp_path):
    put(tmp_path, page(("11111111", "Joining", "Plain.")))
    q8 = {("relay/tenant-purge.md", "Q008", "warning")}
    e = ev(tmp_path, issues_before=q8, issues_after=q8)
    assert ok(s.score_clean_dialect(e, {}))


def test_pages_skip_baseline_copies_and_thread_files(tmp_path):
    put(tmp_path, "# P\n", rel="guides/a.md")
    put(tmp_path, "# old\n", rel=".reflected/guides/a.md")
    put(tmp_path, "# threads\n", rel="guides/a.threads.md")
    assert list(ev(tmp_path).pages("relay")) == ["guides/a.md"]


def test_clean_dialect_is_not_met_by_a_thread_or_a_baseline_alone(tmp_path):
    put(tmp_path, "# PACT\n", rel="PACT.md")
    put(tmp_path, "# threads\n", rel="guides/a.threads.md")
    put(tmp_path, "# old\n", rel=".reflected/guides/a.md")
    e = ev(tmp_path, issues_before=set(), issues_after=set())
    assert "a reflection page has to be written" in [d for g, d in s.score_clean_dialect(e, {}) if not g]


def test_clean_dialect_fails_when_the_pact_was_rewritten(tmp_path):
    put(tmp_path, page(("11111111", "Joining", "Plain.")))
    put(tmp_path, "# PACT\n\n## A\n<!-- r:aaaaaaaa -->\n", rel="PACT.md")
    kw = dict(issues_before=set(), issues_after=set())
    same = ev(tmp_path, pacts_before={"relay": "# PACT\n\n## A\n<!-- r:aaaaaaaa -->\n"}, **kw)
    assert ok(s.score_clean_dialect(same, {}))
    changed = ev(tmp_path, pacts_before={"relay": "# PACT\n\n## A\n<!-- r:aaaaaaaa -->\n\n- x\n"}, **kw)
    assert not ok(s.score_clean_dialect(changed, {}))
    gone = ev(tmp_path, pacts_before={"quiet": "# Q\n"}, **kw)
    assert not ok(s.score_clean_dialect(gone, {}))


def test_clean_dialect_needs_a_written_page(tmp_path):
    put(tmp_path, "# PACT\n", rel="PACT.md")
    e = ev(tmp_path, issues_before=set(), issues_after=set())
    assert not ok(s.score_clean_dialect(e, {}))


# ── dispatcher and data ───────────────────────────────────────────────────

def test_score_evidence_counts_failed_checks_and_unknown_classes(tmp_path):
    put(tmp_path, page(("33333333", "Transports", TABLE), cites={"33333333": ["transport-options"]}))
    e = ev(tmp_path)
    assert s.score_evidence({"checks": ["table_for_comparison"]}, e)[0] == 1
    hard, soft, failed = s.score_evidence({"checks": ["table_for_comparison", "nope"]}, e)
    assert (hard, soft) == (0, 0.5) and "unknown check class 'nope'" in failed[0]
    assert s.score_evidence({}, e)[0] == 0


def test_every_item_names_known_classes():
    root = Path(__file__).resolve().parents[2] / "data/presentation_split"
    total = 0
    for split in ("train", "val", "test"):
        items = json.loads((root / split / "items.json").read_text(encoding="utf-8"))
        total += len(items)
        for item in items:
            assert item["workspace"] == "beacon"
            assert set(item["expect"]["checks"]) <= set(s.SCORERS)
            assert s.describe_expectations(item["expect"])
    assert total == 12
