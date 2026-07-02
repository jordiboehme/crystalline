---
type: schema
title: Task Schema
permalink: domain-bad/schemas/task-schema
tags:
- schema
status: current
recorded_at: 2026-02-20
timestamp: 2026-02-20T00:00:00+00:00
entity: task
version: 1
schema:
  summary: string, one line summary
  priority(enum):
  - low
  - medium
  - high
  estimate?: integer
  part_of: Note
settings:
  validation: warn
  frontmatter:
    owner: string
    reviewers?(array): string
    status(enum):
    - current
    - draft
    - deprecated
---

# Task Schema

Schema for task engrams in this domain, used to exercise the S020-S033
conformance rules.

## Observations

- [requirement] Task engrams carry a one line summary and a priority #schema
