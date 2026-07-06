"""Build the seed skill for training runs.

The trainable document is the body of skills/crystalline-routing/SKILL.md
with the YAML frontmatter stripped: the frontmatter (name, description)
is harness install metadata, not prompt content, and the optimizer must
never edit it. The body is written to outputs/seed_routing.md, which the
training config points at via env.skill_init.
"""
from __future__ import annotations

from pathlib import Path

HARNESS_ROOT = Path(__file__).resolve().parent
SKILL_MD = HARNESS_ROOT.parent.parent / "skills" / "crystalline-routing" / "SKILL.md"
SEED_PATH = HARNESS_ROOT / "outputs" / "seed_routing.md"
EMPTY_PATH = HARNESS_ROOT / "outputs" / "empty_skill.md"


def strip_frontmatter(text: str) -> str:
    if not text.startswith("---\n"):
        return text
    end = text.find("\n---\n", 4)
    if end < 0:
        return text
    return text[end + len("\n---\n"):].lstrip("\n")


def make_seed() -> Path:
    body = strip_frontmatter(SKILL_MD.read_text(encoding="utf-8"))
    SEED_PATH.parent.mkdir(parents=True, exist_ok=True)
    SEED_PATH.write_text(body, encoding="utf-8")
    EMPTY_PATH.write_text("", encoding="utf-8")
    return SEED_PATH


def ensure_prompts() -> None:
    """Copy the vendored optimizer prompts into the installed package.

    The skillopt 0.2.0 wheel ships without its prompt markdown files
    (see vendor/README.md); load_prompt only reads from the package
    directory, so the vendored copies are installed there on startup.
    """
    import shutil

    import skillopt.prompts as prompts_pkg

    target = Path(prompts_pkg.__file__).parent
    vendor = HARNESS_ROOT / "vendor" / "skillopt-prompts"
    for src in sorted(vendor.glob("*.md")):
        dst = target / src.name
        if not dst.exists():
            shutil.copyfile(src, dst)


if __name__ == "__main__":
    print(make_seed())
