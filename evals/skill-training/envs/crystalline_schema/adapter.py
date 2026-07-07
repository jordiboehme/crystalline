"""SkillOpt environment adapter for crystalline schema tasks."""
from __future__ import annotations

from skillopt.datasets.base import BatchSpec
from skillopt.envs.base import EnvAdapter

from envs.crystalline_routing.adapter import _resolve
from envs.crystalline_schema.dataloader import CrystallineSchemaDataLoader
from envs.crystalline_schema.rollout import run_batch


class CrystallineSchemaAdapter(EnvAdapter):
    def __init__(
        self,
        split_dir: str = "",
        data_path: str = "",
        split_mode: str = "split_dir",
        split_ratio: str = "2:1:7",
        split_seed: int = 42,
        split_output_dir: str = "",
        workers: int = 4,
        analyst_workers: int = 4,
        failure_only: bool = False,
        minibatch_size: int = 4,
        edit_budget: int = 4,
        seed: int = 42,
        limit: int = 0,
        claude_bin: str = "claude",
        claude_model: str = "claude-haiku-4-5-20251001",
        crystalline_bin: str = "",
        fixture_root: str = "",
        max_turns: int = 16,
        exec_timeout: int = 300,
    ) -> None:
        self.workers = workers
        self.analyst_workers = analyst_workers
        self.failure_only = failure_only
        self.minibatch_size = minibatch_size
        self.edit_budget = edit_budget
        self.claude_bin = claude_bin
        self.claude_model = claude_model
        self.crystalline_bin = _resolve(
            crystalline_bin, "../../target/release/crystalline"
        )
        self.fixture_root = _resolve(fixture_root, "fixtures/workspaces")
        self.max_turns = max_turns
        self.exec_timeout = exec_timeout
        self.dataloader = CrystallineSchemaDataLoader(
            split_dir=_resolve(split_dir, "data/schema_split"),
            data_path=data_path,
            split_mode=split_mode,
            split_ratio=split_ratio,
            split_seed=split_seed,
            split_output_dir=split_output_dir,
            seed=seed,
            limit=limit,
        )

    def setup(self, cfg: dict) -> None:
        super().setup(cfg)
        self.dataloader.setup(cfg)

    def get_dataloader(self):
        return self.dataloader

    def build_env_from_batch(self, batch: BatchSpec, **kwargs):
        return list(batch.payload or [])

    def build_train_env(self, batch_size: int, seed: int, **kwargs):
        batch = self.dataloader.build_train_batch(
            batch_size=batch_size, seed=seed, **kwargs
        )
        return self.build_env_from_batch(batch, **kwargs)

    def build_eval_env(self, env_num: int, split: str, seed: int, **kwargs):
        batch = self.dataloader.build_eval_batch(
            env_num=env_num, split=split, seed=seed, **kwargs
        )
        return self.build_env_from_batch(batch, **kwargs)

    def rollout(
        self,
        env_manager,
        skill_content: str,
        out_dir: str,
        **kwargs,
    ) -> list[dict]:
        items: list[dict] = env_manager
        return run_batch(
            items=items,
            skill_content=skill_content,
            out_root=out_dir,
            claude_bin=self.claude_bin,
            claude_model=self.claude_model,
            crystalline_bin=self.crystalline_bin,
            fixture_root=self.fixture_root,
            workers=self.workers,
            max_turns=self.max_turns,
            exec_timeout=self.exec_timeout,
        )

    def get_task_types(self) -> list[str]:
        seen: list[str] = []
        for item in (
            self.dataloader.train_items
            + self.dataloader.val_items
            + self.dataloader.test_items
        ):
            task_type = str(item.get("task_type") or "schema")
            if task_type not in seen:
                seen.append(task_type)
        return seen or ["schema"]
