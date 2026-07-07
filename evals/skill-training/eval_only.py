#!/usr/bin/env python3
"""Evaluate one skill document on the routing benchmark without training.

    uv run eval_only.py --config configs/routing.yaml \
        --skill outputs/seed_routing.md --split valid_unseen \
        --out_root outputs/eval_seed_test

Run seed.py first (or any train.py invocation) to refresh
outputs/seed_routing.md and outputs/empty_skill.md from skills/.
"""
from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from scripts import eval_only as skillopt_eval

from envs.crystalline_capture.adapter import CrystallineCaptureAdapter
from envs.crystalline_routing.adapter import CrystallineRoutingAdapter
from envs.crystalline_schema.adapter import CrystallineSchemaAdapter
from seed import ensure_prompts, make_seed


def main() -> None:
    make_seed()
    ensure_prompts()
    skillopt_eval._ENV_REGISTRY["crystalline_routing"] = CrystallineRoutingAdapter
    skillopt_eval._ENV_REGISTRY["crystalline_capture"] = CrystallineCaptureAdapter
    skillopt_eval._ENV_REGISTRY["crystalline_schema"] = CrystallineSchemaAdapter
    skillopt_eval.main()


if __name__ == "__main__":
    main()
