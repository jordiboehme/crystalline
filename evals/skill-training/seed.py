"""Build the seed skills for training runs.

The trainable document is a SKILL.md body with the YAML frontmatter
stripped: the frontmatter (name, description) is harness install
metadata, not prompt content, and the optimizer must never edit it. The
bodies are written under outputs/, where the training configs point via
env.skill_init.
"""
from __future__ import annotations

from pathlib import Path

HARNESS_ROOT = Path(__file__).resolve().parent
SKILLS_DIR = HARNESS_ROOT.parent.parent / "skills"
SEEDS = {
    "crystalline-routing": HARNESS_ROOT / "outputs" / "seed_routing.md",
    "crystalline-capture": HARNESS_ROOT / "outputs" / "seed_capture.md",
}
EMPTY_PATH = HARNESS_ROOT / "outputs" / "empty_skill.md"


def strip_frontmatter(text: str) -> str:
    if not text.startswith("---\n"):
        return text
    end = text.find("\n---\n", 4)
    if end < 0:
        return text
    return text[end + len("\n---\n"):].lstrip("\n")


def make_seed() -> Path:
    EMPTY_PATH.parent.mkdir(parents=True, exist_ok=True)
    for skill_name, seed_path in SEEDS.items():
        source = SKILLS_DIR / skill_name / "SKILL.md"
        body = strip_frontmatter(source.read_text(encoding="utf-8"))
        seed_path.write_text(body, encoding="utf-8")
    EMPTY_PATH.write_text("", encoding="utf-8")
    return SEEDS["crystalline-routing"]


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
