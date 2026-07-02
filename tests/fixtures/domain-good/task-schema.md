---
type: schema
title: Task Schema
permalink: domain-good/schemas/task-schema
tags:
- schema
status: current
recorded_at: 2026-01-07
timestamp: 2026-01-07T08:00:00+00:00
entity: task
version: 1
schema:
  summary: string, one line summary
  priority(enum):
  - low
  - medium
  - high
  estimate?: integer
  blocked_by?(array): Task
settings:
  validation: warn
  frontmatter:
    owner: string
---

# Task Schema

Schema for task engrams in this domain.

## Observations

- [requirement] Task engrams carry a one line summary and a priority #schema
