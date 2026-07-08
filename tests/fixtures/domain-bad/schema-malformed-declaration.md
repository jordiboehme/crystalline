---
type: schema
title: Malformed Declaration Schema
permalink: schemas/malformed-declaration-schema
tags:
- schema
status: current
recorded_at: 2026-02-18
timestamp: 2026-02-18T00:00:00+00:00
entity: widget
version: 1
schema:
  summary: string, one line summary
  bad_field(weird): string
settings:
  validation: sometimes
---

# Malformed Declaration Schema

`bad_field(weird)` uses an unrecognized modifier, and `settings.validation`
is not one of `warn`, `strict` or `off`.

- [note] The body is long enough to avoid a content quality issue.
