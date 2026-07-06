# Vendored files

`skillopt-prompts/` holds the generic optimizer prompts from the
[SkillOpt repository](https://github.com/microsoft/SkillOpt) at tag
v0.2.0 (MIT license). The 0.2.0 wheel on PyPI ships the Python package
without its prompt markdown files, so `load_prompt` raises
FileNotFoundError on every reflect call; the entrypoints copy these
files into the installed package's `skillopt/prompts/` directory at
startup (idempotent, survives a fresh `uv sync`). Drop this vendor dir
once a fixed release packages the prompts.
