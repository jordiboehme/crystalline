"""Dataloader for collaboration tasks: the base items.json reader plus
reference_text synthesis from each item's expect object, so the reflect
stage sees what correct collaboration behavior would have been."""
from __future__ import annotations

from skillopt.datasets.base import SplitDataLoader

from envs.crystalline_collaboration.scoring import describe_expectations


class CrystallineCollaborationDataLoader(SplitDataLoader):
    def load_split_items(self, split_path: str) -> list[dict]:
        items = super().load_split_items(split_path)
        for item in items:
            if "reference_text" not in item:
                item["reference_text"] = describe_expectations(
                    item.get("expect", {}) or {}
                )
        return items
