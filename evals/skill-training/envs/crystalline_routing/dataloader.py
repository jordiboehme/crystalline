"""Dataloader for crystalline routing tasks.

Items live as one JSON array per split directory (items.json), the
format the base SplitDataLoader already reads. On load, every item gets
a reference_text synthesized from its expect object so the reflection
stage can show the optimizer what correct routing would have been,
without the target agent ever seeing it.
"""
from __future__ import annotations

from skillopt.datasets.base import SplitDataLoader

from envs.crystalline_routing.scoring import describe_expectations


class CrystallineRoutingDataLoader(SplitDataLoader):
    def load_split_items(self, split_path: str) -> list[dict]:
        items = super().load_split_items(split_path)
        for item in items:
            if "reference_text" not in item:
                item["reference_text"] = describe_expectations(
                    item.get("expect", {}) or {}
                )
        return items
