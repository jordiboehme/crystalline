---
type: manifest
title: MANIFEST
permalink: manifest
tags:
- manifest
- entry-point
status: current
recorded_at: 2026-09-07
timestamp: 2026-09-07T09:00:00+00:00
---

# Smoke Private Domain

The domain `fluid/e2e/members.spec.ts` closes, invites an account into and
opens again. It holds nothing but this manifest: what that journey reads is
who may see the domain at all, never anything inside it.

## Scope

- A domain the members journey can make private without touching the domain
  every other journey browses

## When to Use

- When the browser smoke needs a domain whose membership it can change
- Never in a real installation; this is test content

## Notes for Agents

- Adding an engram here is safe but pointless; the journey asserts on the
  domain's own visibility, not on its content.
