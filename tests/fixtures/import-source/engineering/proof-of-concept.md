---
type: poc
title: Batched Widget Import Proof of Concept
permalink: import-source/engineering/proof-of-concept
tags:
  - poc
  - widget
status: current
recorded_at: 2026-02-20
---

# Batched Widget Import Proof of Concept

This proof of concept batches widget creation requests into a single
transaction to cut round trips during a bulk import.

Early measurements show a meaningful reduction in total import time for
large batches.
