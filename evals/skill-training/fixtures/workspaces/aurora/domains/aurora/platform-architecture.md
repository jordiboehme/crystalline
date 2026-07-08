---
type: architecture
title: Platform architecture
permalink: platform-architecture
tags:
- aurora
- architecture
status: draft
recorded_at: 2026-07-07
timestamp: 2026-07-07T17:29:48.944407+00:00
---

An early sketch of the Aurora platform. As a working illustration this overview shows scenario budgeting as the first capability and twelve source connectors at launch; the strategy narrative holds the committed scope. Four layers move signals from ingestion to reasoning and on to reallocation decisions.

The layers below are stable even where the capability list above is not.

- [fact] The ingestion layer normalizes execution signals from work trackers into the outcome ledger #architecture
- [fact] The outcome ledger is the platform's core asset: an append-only record linking each decision to what actually happened #architecture
- [fact] The reasoning layer replays past decisions against the ledger to price each reallocation move #architecture
- refines [[Strategy narrative]]
