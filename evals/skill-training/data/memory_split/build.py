"""Build the memory benchmark split from the routing and capture splits.

The memory skill is the consolidated recall-plus-capture skill, so its
benchmark is a stratified blend of the two source benchmarks: every item
keeps its original expect vocabulary and gains a `bench` field the env
uses to dispatch scoring. Train and val sample both sources evenly per
task type; test keeps both source test splits whole so memory numbers
stay directly comparable to the per-skill baselines.

Regenerate after editing the source splits:

    uv run python data/memory_split/build.py
"""
from __future__ import annotations

import json
import random
from collections import defaultdict
from pathlib import Path

OUT = Path(__file__).resolve().parent
DATA = OUT.parent
BENCHES = ("routing", "capture")
QUOTAS = {"train": 20, "val": 10}
SEED = 42


def load(bench: str, split: str) -> list[dict]:
    path = DATA / f"{bench}_split" / split / "items.json"
    items = json.loads(path.read_text(encoding="utf-8"))
    return [{**item, "bench": bench} for item in items]


def stratified(items: list[dict], quota: int, seed: int) -> list[dict]:
    """A deterministic per-task-type proportional sample.

    Largest-remainder allocation with at least one item per type, so
    every source behavior stays represented in the smaller blend.
    """
    groups: dict[str, list[dict]] = defaultdict(list)
    for item in items:
        groups[str(item.get("task_type"))].append(item)
    names = sorted(groups)
    share = {n: quota * len(groups[n]) / len(items) for n in names}
    counts = {n: min(len(groups[n]), max(1, int(share[n]))) for n in names}
    while sum(counts.values()) > quota:
        n = max(
            (n for n in names if counts[n] > 1),
            key=lambda n: (counts[n] - share[n], n),
        )
        counts[n] -= 1
    while sum(counts.values()) < quota:
        n = max(
            (n for n in names if counts[n] < len(groups[n])),
            key=lambda n: (share[n] - counts[n], n),
        )
        counts[n] += 1
    rng = random.Random(seed)
    picked: list[dict] = []
    for n in names:
        pool = sorted(groups[n], key=lambda item: str(item["id"]))
        picked.extend(rng.sample(pool, counts[n]))
    return picked


def build() -> None:
    for split in ("train", "val", "test"):
        merged: list[dict] = []
        for bench in BENCHES:
            items = load(bench, split)
            quota = QUOTAS.get(split)
            if quota is not None:
                items = stratified(items, quota, SEED)
            merged.extend(items)
        merged.sort(key=lambda item: (item["bench"], str(item["id"])))
        target = OUT / split / "items.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(
            json.dumps(merged, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        print(f"{split}: {len(merged)} items")


if __name__ == "__main__":
    build()
