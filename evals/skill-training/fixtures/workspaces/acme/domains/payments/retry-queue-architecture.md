---
type: architecture
title: Retry queue architecture
permalink: retry-queue-architecture
tags:
- architecture
- payments
status: current
recorded_at: 2026-07-06
timestamp: 2026-07-06T15:08:06.231784+00:00
---

How failed payment jobs are queued and replayed.

- [fact] The retry queue is backed by Redis streams with one consumer group per worker pool #payments
- [fact] Jobs carry an idempotency key so a replay is always safe #payments
