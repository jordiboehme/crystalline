---
type: task
title: Schema Violation Task
permalink: domain-bad/schema-violation-task
tags:
- schema
status: current
recorded_at: 2026-02-21
timestamp: 2026-02-21T00:00:00+00:00
---

# Schema Violation Task

This task implicitly matches the Task Schema by `type`. It is missing the
required `summary` observation and the required `part_of` relation, has an
invalid `priority` value and a non-integer `estimate`, and its frontmatter
is missing the schema-required `owner` field.

## Observations

- [priority] urgent
- [estimate] soon
