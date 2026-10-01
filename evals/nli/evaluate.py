#!/usr/bin/env python3
"""Evaluate one profile of the contradiction check against the planted pairs.

Reads the dump the scale harness writes after an `nli` stage (every stored
line-pair row and every scored engram pair, with texts and cosines) and the
sidecar generate.py writes beside the corpus (every planted pair, flipped or a
hard negative), and reports per model:

- precision and recall for finding lines from 0.50 to 0.99, and the line that
  reaches 0.9 precision, for the mean and for the min of both reading orders;
- a reliability table: predicted probability against observed rate;
- recall and noise at related lines 0.70, 0.75, 0.80 and 0.85 (the stage
  scores at 0.70, so higher lines are read off the stored cosine);
- recall per flip type and per language; the period hint's hit rate on the
  dated pairs, and separately on the version-number flips (the hint fires on
  a dotted version string such as "18.19.0" the same way it fires on a day
  and month, so that hit rate is reported on its own to show the misfire
  rate rather than folding it into the dated-pair rate);
- the planted flips that reach 0.70 under min (`flips_at_0_70`), recall at
  higher line floors read off the stored `similarity`, and with `--top N` the
  product's ranked list per domain;
- findings on unplanted pairs (noise), milliseconds per batch of 16, line
  pairs per second, the drain's wall time and the resident size plateau.

    python3 evals/nli/evaluate.py --dump evals/scale/out/nli-full.json \\
        --sidecar evals/scale/corpus/contradictions.json \\
        --log /tmp/crystalline-scale/crystalline/daemon.log \\
        --rss evals/scale/out/nli-full-rss.csv
    python3 evals/nli/evaluate.py --self-test
"""

import argparse
import csv
import json
import re
import sys
from pathlib import Path

THRESHOLDS = [round(0.50 + i * 0.01, 2) for i in range(50)]
RELATED = [0.70, 0.75, 0.80, 0.85]
BINS = [0.5, 0.6, 0.7, 0.8, 0.9, 1.01]
FLOORS = [0.84, 0.86, 0.88, 0.90, 0.94]

# tracing-subscriber writes ANSI color codes into daemon.log unless NO_COLOR
# is set; the run.sh isolation block sets it, but a log written by an older
# run or a caller that dropped the env var still has to parse.
ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")


def combine(ab: float, ba: float, how: str) -> float:
    # The store keeps both orders (score_ba is NOT NULL); a missing one reads
    # as 0.0 so an older or hand-made dump cannot raise.
    ab = 0.0 if ab is None else ab
    ba = 0.0 if ba is None else ba
    return (ab + ba) / 2 if how == "mean" else min(ab, ba)


def key(domain: str, a: str, b: str) -> tuple:
    return (domain, *sorted((a, b)))


def planted_scores(sidecar: list, dump: dict) -> list:
    """Every planted item with its pair cosine (None: never a candidate) and
    the two raw scores of its planted line pair (None: below the store floor)."""
    pairs = {key(p["domain"], p["a"], p["b"]): p["cosine"] for p in dump["pairs"]}
    rows = {}
    for r in dump["rows"]:
        k = key(r["domain"], r["a"], r["b"])
        rows[(k, r["a"], r["line_a"], r["b"], r["line_b"])] = r
    out = []
    for item in sidecar:
        k = key(item["domain"], item["a"], item["b"])
        row = rows.get((k, item["a"], item["a_line"], item["b"], item["b_line"])) or rows.get(
            (k, item["b"], item["b_line"], item["a"], item["a_line"])
        )
        out.append(
            {
                **item,
                "cosine": pairs.get(k),
                "score_ab": row["score_ab"] if row else None,
                "score_ba": row["score_ba"] if row else None,
                "similarity": row.get("similarity") if row else None,
                "period": bool(row["period"]) if row else False,
            }
        )
    return out


def predicted(item: dict, threshold: float, related: float, how: str) -> bool:
    if item["cosine"] is None or item["cosine"] < related or item["score_ab"] is None:
        return False
    return combine(item["score_ab"], item["score_ba"], how) >= threshold


def flips_at(items: list, threshold: float = 0.70, related: float = 0.80) -> int:
    """Planted flips whose min of both orders reaches `threshold` on a pair at
    or above `related`: the line-filter gate's count."""
    return sum(predicted(i, threshold, related, "min") for i in items if i["kind"] == "flip")


def ranked(dump: dict, threshold: float = 0.70, top: int = 10) -> dict:
    """Per domain, one entry per engram pair: its strongest line pair by the
    min of both orders at or above `threshold`, how many more of its line
    pairs reach it, highest first, the first `top`. The product's own V302
    ranking, without acknowledgments."""
    groups = {}
    for r in dump["rows"]:
        s = combine(r["score_ab"], r["score_ba"], "min")
        if s < threshold:
            continue
        groups.setdefault((r["domain"], *sorted((r["a"], r["b"]))), []).append((s, r))
    out = {}
    for (domain, a, b), items in groups.items():
        items.sort(key=lambda x: (-x[0], x[1]["line_a"], x[1]["line_b"]))
        s, best = items[0]
        out.setdefault(domain, []).append({
            "a": best["a"], "b": best["b"], "score": s, "similarity": best.get("similarity"),
            "line_a": best["line_a"], "line_b": best["line_b"],
            "a_text": best.get("a_text"), "b_text": best.get("b_text"), "more": len(items) - 1,
        })
    for domain in out:
        out[domain].sort(key=lambda e: (-e["score"], e["a"], e["b"]))
        out[domain] = out[domain][:top]
    return out


def precision_recall(items: list, threshold: float, related: float, how: str) -> tuple:
    flips = [i for i in items if i["kind"] == "flip"]
    negatives = [i for i in items if i["kind"] == "negative"]
    tp = sum(predicted(i, threshold, related, how) for i in flips)
    fp = sum(predicted(i, threshold, related, how) for i in negatives)
    precision = tp / (tp + fp) if tp + fp else 1.0
    recall = tp / len(flips) if flips else 0.0
    return precision, recall


def noise(dump: dict, planted: list, threshold: float, related: float, how: str) -> dict:
    """Findings on line pairs nobody planted, split: probe against probe (two
    probes can share a subject, so some are real) and natural corpus rows."""
    lines = {
        (key(i["domain"], i["a"], i["b"]), frozenset({(i["a"], i["a_line"]), (i["b"], i["b_line"])}))
        for i in planted
    }
    count = {"probes": 0, "natural": 0}
    for r in dump["rows"]:
        k = key(r["domain"], r["a"], r["b"])
        if (k, frozenset({(r["a"], r["line_a"]), (r["b"], r["line_b"])})) in lines:
            continue
        if r["cosine"] >= related and combine(r["score_ab"], r["score_ba"], how) >= threshold:
            count["probes" if r["domain"] == "probes" else "natural"] += 1
    return count


def speed(log_path) -> dict:
    if not log_path or not Path(log_path).exists():
        return {}
    lines = pairs = batches = ms = 0
    embedded = embedded_ms = 0
    for raw in Path(log_path).read_text(encoding="utf-8", errors="replace").splitlines():
        line = ANSI_RE.sub("", raw)
        if "embedded observation lines for the contradiction check" in line:
            m_lines = re.search(r"\blines=(\d+)", line)
            m_lms = re.search(r"\bms=(\d+)", line)
            if m_lines and m_lms:
                embedded += int(m_lines.group(1))
                embedded_ms += int(m_lms.group(1))
            continue
        if "scored related pairs for possible contradictions" not in line:
            continue
        m_pairs = re.search(r"\bline_pairs=(\d+)", line)
        m_batches = re.search(r"\bbatches=(\d+)", line)
        m_ms = re.search(r"\bms=(\d+)", line)
        if not (m_pairs and m_batches and m_ms):
            continue
        lines += 1
        pairs += int(m_pairs.group(1))
        batches += int(m_batches.group(1))
        ms += int(m_ms.group(1))
    line_embedding = {"line_embedding": {"lines": embedded, "ms": embedded_ms}}
    if not batches:
        return line_embedding if embedded else {}
    return {
        **line_embedding,
        "passes": lines,
        "line_pairs": pairs,
        "ms_per_batch": ms / batches,
        "line_pairs_per_second": pairs / (ms / 1000) if ms else None,
    }


def rss(path) -> dict:
    if not path or not Path(path).exists():
        return {}
    samples = [r for r in csv.DictReader(open(path, encoding="utf-8")) if r["rss_mb"]]
    loaded = [float(r["rss_mb"]) for r in samples if r.get("loaded") == "true"]
    return {
        "plateau_mb": max(loaded) if loaded else None,
        "after_unload_mb": float(samples[-1]["rss_mb"]) if samples else None,
    }


def report(dump: dict, sidecar: list, langs=None, log=None, rss_path=None) -> dict:
    items = [i for i in planted_scores(sidecar, dump) if langs is None or i["lang"] in langs]
    out = {"profile": dump.get("profile"), "model": dump.get("model"), "planted": len(items)}
    # Coverage, split from recall: a planted item can miss for reasons that
    # are not the model. "uncandidated" never cleared the related-pair cosine
    # line, or lost a slot to the pair cap before scoring ever started
    # (`cosine` is None). "no_stored_row" cleared it but has no contradiction
    # row: on a fully drained dump that means the pair WAS scored and correctly
    # landed below CONTRADICTION_STORE_FLOOR (0.5) - the store never persists a
    # low score, so a negative probe scoring low leaves no row and is expected
    # to, not a gap. On a dump that did not finish draining the two cases are
    # indistinguishable from here (still pending vs. scored-and-floored both
    # look like "no row"), which `markdown()` says explicitly. Correction 24's
    # worry is exactly this: a recall number that is actually measuring the
    # cap needs to be told apart from one measuring the model, and this is how
    # a report shows the difference instead of hiding it inside a lower recall
    # number.
    out["coverage"] = {
        "uncandidated": {k: sum(i["kind"] == k and i["cosine"] is None for i in items) for k in ("flip", "negative")},
        "no_stored_row": {
            k: sum(i["kind"] == k and i["cosine"] is not None and i["score_ab"] is None for i in items)
            for k in ("flip", "negative")
        },
    }
    out["planted_flips"] = sum(i["kind"] == "flip" for i in items)
    out["flips_at_0_70"] = flips_at(items)
    # Rows are stored only at or above the product floor, so higher floors are
    # read off the stored similarity.
    out["line_floors"] = [
        {"floor": f, "recall": sum(
            predicted(i, 0.70, 0.80, "min") and (i["similarity"] or 0) >= f
            for i in items if i["kind"] == "flip") / max(1, out["planted_flips"])}
        for f in FLOORS
    ]
    for how in ("mean", "min"):
        sweep = []
        for t in THRESHOLDS:
            p, r = precision_recall(items, t, 0.80, how)
            sweep.append({"threshold": t, "precision": p, "recall": r, "noise": noise(dump, items, t, 0.80, how)})
        at_09 = next((s["threshold"] for s in sweep if s["precision"] >= 0.9), None)
        bins = []
        for lo, hi in zip(BINS, BINS[1:]):
            inside = [
                i for i in items
                if i["score_ab"] is not None and lo <= combine(i["score_ab"], i["score_ba"], how) < hi
            ]
            flips = sum(i["kind"] == "flip" for i in inside)
            bins.append({"from": lo, "to": min(hi, 1.0), "pairs": len(inside), "observed": flips / len(inside) if inside else None})
        line = at_09 if at_09 is not None else 0.85
        related = []
        for rl in RELATED:
            p, r = precision_recall(items, line, rl, how)
            related.append({"related": rl, "precision": p, "recall": r, "noise": noise(dump, items, line, rl, how)})
        by = lambda field: {  # noqa: E731
            v: precision_recall([i for i in items if i[field] == v or i["kind"] == "negative"], line, 0.80, how)[1]
            for v in sorted({i[field] for i in items if i["kind"] == "flip"})
        }
        dated = [i for i in items if i["type"] in ("date", "period-dated")]
        versioned = [i for i in items if i["type"] == "version"]
        out[how] = {
            "sweep": sweep,
            "threshold_at_precision_0_9": at_09,
            "reliability": bins,
            "related": related,
            "recall_by_type": by("type"),
            "recall_by_lang": by("lang"),
            "period_hint_hit_rate": sum(i["period"] for i in dated) / len(dated) if dated else None,
            "period_hint_hit_rate_versions": sum(i["period"] for i in versioned) / len(versioned) if versioned else None,
        }
    out["speed"] = speed(log)
    out["rss"] = rss(rss_path)
    out["wall_seconds"] = dump.get("wall_seconds")
    # Absent (an older dump) reads as fully drained, never as partial by
    # default: a dump that stopped early always says so explicitly.
    out["drained"] = dump.get("drained", True)
    return out


def markdown(r: dict) -> str:
    lines = [f"## {r['profile']} ({r['model']})", "", f"planted pairs: {r['planted']}", ""]
    if r.get("drained", True):
        no_row_note = (
            "on a negative this means the model correctly scored it below the store floor (0.5), not a gap; "
            "on a flip it means the model missed it - a real recall loss, not a cap or threshold effect"
        )
    else:
        no_row_note = "the drain did not finish, so this may still be pending rather than scored below the floor"
    lines += [f"coverage (missed the cap or the related-pair line, never a candidate): {r['coverage']['uncandidated']}",
              f"coverage (candidate, no stored row - {no_row_note}): {r['coverage']['no_stored_row']}", ""]
    if not r.get("drained", True):
        lines += ["**PARTIAL: the backlog had not drained within DRAIN_LIMIT; every number below undercounts.**", ""]
    lines += [f"planted flips at or above 0.70 (min of both orders, related 0.80): {r['flips_at_0_70']} of {r['planted_flips']}", "",
              "| line floor (similarity) | recall at 0.70 |", "|---|---|"]
    lines += [f"| {f['floor']:.2f} | {f['recall']:.3f} |" for f in r["line_floors"]] + [""]
    for how in ("mean", "min"):
        m = r[how]
        lines += [f"### {how} of both orders", "", f"line reaching 0.9 precision: {m['threshold_at_precision_0_9']}", ""]
        if how == "mean":
            lines += ["Unreliable since 11c: under Min the second order is read only where the first reaches 0.5, "
                      "so a line pair below that has no second order and no row.", ""]
        lines += ["| line | precision | recall | noise probes | noise natural |", "|---|---|---|---|---|"]
        for s in m["sweep"][::5]:
            lines.append(
                f"| {s['threshold']:.2f} | {s['precision']:.3f} | {s['recall']:.3f} "
                f"| {s['noise']['probes']} | {s['noise']['natural']} |"
            )
        lines += ["", "| related | precision | recall | noise probes | noise natural |", "|---|---|---|---|---|"]
        for s in m["related"]:
            lines.append(
                f"| {s['related']:.2f} | {s['precision']:.3f} | {s['recall']:.3f} "
                f"| {s['noise']['probes']} | {s['noise']['natural']} |"
            )
        lines += ["", f"recall by type: {m['recall_by_type']}", f"recall by language: {m['recall_by_lang']}",
                  f"period hint hit rate: {m['period_hint_hit_rate']}",
                  f"period hint hit rate (version flips): {m['period_hint_hit_rate_versions']}", ""]
    lines += [f"speed: {r['speed']}", f"resident size: {r['rss']}", f"drain wall seconds: {r['wall_seconds']}", ""]
    return "\n".join(lines)


def cut(text, n: int = 120) -> str:
    text = " ".join((text or "").split()).replace("|", "/")
    return text if len(text) <= n else text[: n - 3] + "..."


def top_markdown(top: dict) -> str:
    out = []
    for domain in sorted(top):
        out += [f"### top {len(top[domain])} in {domain}", "",
                "| rank | a | line | text | b | line | text | score | similarity | more |",
                "|---|---|---|---|---|---|---|---|---|---|"]
        for n, e in enumerate(top[domain], 1):
            sim = "" if e["similarity"] is None else f"{e['similarity']:.3f}"
            out.append(f"| {n} | {e['a']} | {e['line_a']} | {cut(e['a_text'])} | {e['b']} | {e['line_b']} "
                       f"| {cut(e['b_text'])} | {e['score']:.3f} | {sim} | {e['more']} |")
        out.append("")
    return "\n".join(out)


def self_test() -> int:
    sidecar = [
        {"id": 0, "kind": "flip", "type": "number", "lang": "en", "domain": "d", "a": "p0a", "b": "p0b", "a_line": 9, "b_line": 9, "a_text": "x", "b_text": "y"},
        {"id": 1, "kind": "flip", "type": "negation", "lang": "de", "domain": "d", "a": "p1a", "b": "p1b", "a_line": 9, "b_line": 9, "a_text": "x", "b_text": "y"},
        {"id": 2, "kind": "negative", "type": "paraphrase", "lang": "en", "domain": "d", "a": "n0a", "b": "n0b", "a_line": 9, "b_line": 9, "a_text": "x", "b_text": "y"},
    ]
    dump = {
        "profile": "full", "model": "m", "wall_seconds": 10,
        "pairs": [
            {"domain": "d", "a": "p0a", "b": "p0b", "cosine": 0.9},
            {"domain": "d", "a": "p1b", "b": "p1a", "cosine": 0.72},
            {"domain": "d", "a": "n0a", "b": "n0b", "cosine": 0.9},
        ],
        "rows": [
            {"domain": "d", "a": "p0a", "b": "p0b", "line_a": 9, "line_b": 9, "score_ab": 0.95, "score_ba": 0.93, "period": 0, "cosine": 0.9, "similarity": 0.93, "a_text": "a_text", "b_text": "b_text"},
            {"domain": "d", "a": "p1b", "b": "p1a", "line_a": 9, "line_b": 9, "score_ab": 0.90, "score_ba": 0.60, "period": 0, "cosine": 0.72, "similarity": 0.88, "a_text": "a_text", "b_text": "b_text"},
            {"domain": "d", "a": "n0a", "b": "n0b", "line_a": 9, "line_b": 9, "score_ab": 0.55, "score_ba": 0.50, "period": 0, "cosine": 0.9, "similarity": 0.90, "a_text": "a_text", "b_text": "b_text"},
            {"domain": "d", "a": "n0a", "b": "n0b", "line_a": 10, "line_b": 10, "score_ab": 0.99, "score_ba": 0.99, "period": 0, "cosine": 0.9, "similarity": 0.95, "a_text": "a_text", "b_text": "b_text"},
        ],
    }
    r = report(dump, sidecar)
    first = r["mean"]["sweep"][35]
    assert first["threshold"] == 0.85, first
    assert first["precision"] == 1.0 and first["recall"] == 0.5, first
    assert first["noise"] == {"probes": 0, "natural": 1}, "the unplanted line pair at 0.99 is natural noise"
    # The 0.9-precision line is 0.53 here (the negative's mean is 0.525), and
    # at that line p1 counts once the related line drops to 0.70.
    assert r["mean"]["threshold_at_precision_0_9"] == 0.53, r["mean"]["threshold_at_precision_0_9"]
    related = {x["related"]: x for x in r["mean"]["related"]}
    assert related[0.70]["recall"] == 1.0, related
    assert related[0.80]["recall"] == 0.5, related
    assert r["min"]["sweep"][35]["recall"] == 0.5
    assert r["mean"]["period_hint_hit_rate_versions"] is None, "no version-type item in the self-test sidecar"
    assert r["coverage"] == {"uncandidated": {"flip": 0, "negative": 0}, "no_stored_row": {"flip": 0, "negative": 0}}, (
        "every planted item here has both a pair cosine and a scored row"
    )
    assert r["planted_flips"] == 2, r["planted_flips"]
    assert r["flips_at_0_70"] == 1, "p0 clears 0.70 under min at related 0.80; p1 sits at related 0.72"
    top = ranked(dump, threshold=0.70, top=10)
    assert [(e["a"], e["b"]) for e in top["d"]] == [("n0a", "n0b"), ("p0a", "p0b")], top
    assert top["d"][0]["more"] == 0 and top["d"][0]["score"] == 0.99
    floors = {f["floor"]: f["recall"] for f in r["line_floors"]}
    assert floors[0.86] == 0.5 and floors[0.90] == 0.5 and floors[0.94] == 0.0, floors
    assert "planted flips at or above 0.70 (min of both orders, related 0.80): 1 of 2" in markdown(r)
    assert "| 1 | n0a | 10 | a_text |" in top_markdown(top)
    print("self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dump", type=Path)
    parser.add_argument("--sidecar", type=Path)
    parser.add_argument("--log", type=Path)
    parser.add_argument("--rss", type=Path)
    parser.add_argument("--langs", help="comma separated, for example en to score the English pairs only")
    parser.add_argument("--out", type=Path)
    parser.add_argument("--top", type=int, default=0, help="also print the product's ranked top N per domain")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.dump or not args.sidecar:
        parser.error("--dump and --sidecar are required")
    dump = json.loads(args.dump.read_text(encoding="utf-8"))
    sidecar = json.loads(args.sidecar.read_text(encoding="utf-8"))
    if any("similarity" not in row for row in dump["rows"]):
        parser.error(f"{args.dump} has rows without a similarity column: it is a stale dump, rebuild it with run.sh --stage nli")
    langs = set(args.langs.split(",")) if args.langs else None
    r = report(dump, sidecar, langs, args.log, args.rss)
    print(markdown(r))
    if args.top > 0:
        print(top_markdown(ranked(dump, top=args.top)))
    out = args.out or args.dump.with_name(args.dump.stem + "-report.json")
    out.write_text(json.dumps(r, indent=2), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
