---
type: engram
title: Domain Prefixed Permalink
permalink: domain-bad/prefixed-example
tags:
- fixtures
status: current
recorded_at: 2026-01-05
timestamp: 2026-01-05T09:05:00+00:00
description: A permalink that repeats the domain name, the E008 antipattern.
---

# Domain Prefixed Permalink

This engram is otherwise clean, but its permalink glues the domain name onto
what should be a domain-relative slug. The domain name is per-user
configuration, so persisting it here misleads as soon as the domain is
registered under another name. E008 flags exactly this.
