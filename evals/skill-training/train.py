#!/usr/bin/env python3
"""Train a crystalline skill with SkillOpt.

Thin entrypoint: SkillOpt keeps its env registry in its train script
rather than the package, so this file registers the crystalline_routing
adapter there, refreshes the seed skill from skills/ and delegates. The
CLI is SkillOpt's own:

    uv run train.py --config configs/routing.yaml
    uv run train.py --config configs/routing.yaml \
        --cfg-options env.limit=4 train.batch_size=4 train.num_epochs=1
"""
from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from scripts import train as skillopt_train

from envs.crystalline_routing.adapter import CrystallineRoutingAdapter
from seed import ensure_prompts, make_seed


def main() -> None:
    make_seed()
    ensure_prompts()
    skillopt_train._ENV_REGISTRY["crystalline_routing"] = CrystallineRoutingAdapter
    skillopt_train.main()


if __name__ == "__main__":
    main()
