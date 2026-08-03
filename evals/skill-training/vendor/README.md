# Vendored files

`skillopt-prompts/` holds the generic optimizer prompts from the
[SkillOpt repository](https://github.com/microsoft/SkillOpt) at tag
v0.2.0 (MIT license). The 0.2.0 wheel on PyPI ships the Python package
without its prompt markdown files, so `load_prompt` raises
FileNotFoundError on every reflect call; the entrypoints copy these
files into the installed package's `skillopt/prompts/` directory at
startup (idempotent, survives a fresh `uv sync`). Drop this vendor dir
once a fixed release packages the prompts.

Upstream tracking (checked 2026-08-03): this is SkillOpt issue #117,
fixed by PR #135 and follow-up #137 on 2026-07-14 with three lines of
`[tool.setuptools.package-data]` in `pyproject.toml`. The fix sits under
`## [Unreleased]`, so there is still no release carrying it: 0.2.0
(2026-07-02) remains the newest on PyPI and on GitHub, even though
`main` is far ahead of it. Bumping the `skillopt[claude]>=0.2.0` pin
therefore changes nothing.

The vendored copies are current, not stale: every file here was hashed
against upstream `main` on 2026-08-03 and all 21 match, `slow_update.md`
included. When a release does land, confirm the prompts ship with
`grep -c '\.md' .../skillopt-<version>.dist-info/RECORD` returning
non-zero, then delete this directory along with `ensure_prompts()` and
its two call sites in `train.py` and `eval_only.py`. That removal is
safe to do later rather than immediately, since `ensure_prompts()` only
copies when the destination is missing, so a fixed wheel's own files
already win.
