---
type: engram
title: Postgres connection gotcha
permalink: postgres-connection-gotcha
tags:
- gotcha
- infra
status: current
recorded_at: 2026-07-06
timestamp: 2026-07-06T15:08:06.296502+00:00
---

A database limit that bites under load.

- [gotcha] pgbouncer caps client connections at 400 per pool; raising it needs a coordinated restart #infra
- [fact] Connection pool sizing is owned by the platform team #infra
