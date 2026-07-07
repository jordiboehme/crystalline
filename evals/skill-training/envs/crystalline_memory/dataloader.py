"""Dataloader for the memory benchmark: combined routing and capture
items, each carrying a `bench` field that picks the scorer. The
reference_text comes from the matching bench's expectation describer."""
from __future__ import annotations

from skillopt.datasets.base import SplitDataLoader

from envs.crystalline_capture import scoring as capture_scoring
from envs.crystalline_routing import scoring as routing_scoring

DESCRIBERS = {
    "routing": routing_scoring.describe_expectations,
    "capture": capture_scoring.describe_expectations,
}


def bench_of(item: dict) -> str:
    bench = str(item.get("bench") or "")
    if bench not in DESCRIBERS:
        raise ValueError(
            f"memory item {item.get('id')!r} needs a bench field "
            f"of {sorted(DESCRIBERS)}, got {bench!r}"
        )
    return bench


class CrystallineMemoryDataLoader(SplitDataLoader):
    def load_split_items(self, split_path: str) -> list[dict]:
        items = super().load_split_items(split_path)
        for item in items:
            describe = DESCRIBERS[bench_of(item)]
            if "reference_text" not in item:
                item["reference_text"] = describe(item.get("expect", {}) or {})
        return items
