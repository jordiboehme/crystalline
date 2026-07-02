---
type: schema
title: Task Schema
permalink: product/schemas/task-schema
tags:
- schema
- product
status: current
recorded_at: 2026-04-01
valid_from: 2026-04-01
entity: task
version: 1
schema:
  summary: string, one line summary
  priority(enum):
  - low
  - medium
  - high
  estimate?: integer
  requirement?(array): string
  blocked_by?(array): Task
  part_of?: Note
settings:
  validation: warn
  frontmatter:
    status(enum):
    - current
    - draft
    - deprecated
    owner: string
    reviewers?(array): string
---

# Task Schema

Schema for task engrams in the product domain.

## Observations

- [requirement] Task engrams carry a one line summary and a priority #schema
