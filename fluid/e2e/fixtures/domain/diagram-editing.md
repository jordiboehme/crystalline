---
type: guide
title: Flowchart by Hand
permalink: diagram-editing
tags:
- smoke
status: current
recorded_at: 2026-02-04
timestamp: 2026-02-04T10:00:00+00:00
description: Diagrams for the journeys that change a flowchart by its boxes and arrows.
---

Text above the flowchart.
```mermaid
flowchart TD
  %% keep this comment
  A[Start]   -->   B[Middle] --> C[End]
  C -->|done|   D[Other]
  D    --> E[Last]
```

Text between the fences.

```mermaid
stateDiagram-v2
  [*] --> Waiting
  Waiting --> Running
  Running --> [*]
```

> A quoted flowchart.
>
> ```mermaid
> flowchart LR
>   Q[Quoted] --> R[Reply]
> ```

Text at the end.
