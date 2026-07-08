---
type: task
title: Schema Violation Task Two
permalink: schema-violation-task-two
tags:
- schema
status: proposed
recorded_at: 2026-02-22
timestamp: 2026-02-22T00:00:00+00:00
owner: 42
reviewers: dana
---

# Schema Violation Task Two

`owner` above is an integer instead of a string, `reviewers` is a plain
string instead of a list, and `status` (`proposed`) is outside this
schema's own frontmatter enum even though it is otherwise a recommended
status value.

## Observations

- [summary] Rebalance the seedling grow-light schedule
- [priority] low

## Relations

- part_of [[MANIFEST]]
