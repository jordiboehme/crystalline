---
type: guide
title: Lantern Protocol
permalink: lantern-protocol
tags:
- protocol
- smoke
status: current
recorded_at: 2026-02-03
timestamp: 2026-02-03T10:00:00+00:00
description: How a harbor lantern is handed from one watch to the next.
---

# Lantern Protocol

A lantern is handed over at the end of every watch, and the handover is
written down before the light changes hands. The diagram below is the whole
of it, and it is here so the browser smoke has a fence to render.

```mermaid
flowchart TD
    A[Watch ends] --> B[Log the reading]
    B --> C[Hand the lantern over]
    C --> D[Next watch signs]
```

## Observations

- [requirement] The reading is logged before the lantern changes hands #protocol
- [gotcha] An unsigned handover is not a handover #protocol

## Relations

- relates_to [[Deep Gamma Note]]
- relates_to [[Harbor Signal Log]]
