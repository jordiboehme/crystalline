---
type: manifest
title: MANIFEST
permalink: manifest
tags:
- manifest
- entry-point
status: current
recorded_at: 2026-02-02
timestamp: 2026-02-02T09:00:00+00:00
---

# Fluid Smoke Domain

The knowledge the browser smoke reads. Small on purpose: every engram in here
exists because one assertion in `fluid/e2e/smoke.spec.ts` depends on it.

## Scope

- A diagram fence, so a rendered SVG can be asserted on
- An engram in a subfolder, so a multi-segment permalink can be deep linked
- Relations between the engrams, so a neighborhood has arrows to draw
- Distinct titles and tags, so a search and the palette have something to find

## When to Use

- When the browser smoke needs a domain to browse
- Never in a real installation; this is test content

## Notes for Agents

- Change an engram here only together with the assertion that reads it.
