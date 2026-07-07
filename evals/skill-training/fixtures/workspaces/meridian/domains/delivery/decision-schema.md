---
type: schema
title: Decision Schema
permalink: decision-schema
tags:
- schema
- delivery
status: current
recorded_at: 2026-07-07
timestamp: 2026-07-07T05:06:43.612122+00:00
entity: decision
schema:
  priority(enum):
  - low
  - medium
  - high
  rationale?: string
  summary: string, one line summary of the decision
  supersedes?: Decision
settings:
  frontmatter:
    owner: string
  validation: warn
version: 1
---

The shape for decision engrams in this domain.

- [convention] Decisions carry a one line summary and a priority #delivery
- [convention] The deciding owner is recorded in frontmatter #delivery
