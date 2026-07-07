---
type: schema
title: Playbook Schema
permalink: playbook-schema
tags:
- schema
- delivery
status: current
recorded_at: 2026-07-07
timestamp: 2026-07-07T05:06:43.656328+00:00
entity: playbook
schema:
  objective: string, what the playbook achieves
  step(array): string
settings:
  frontmatter:
    owner: string
  validation: strict
version: 1
---

The shape for playbook engrams in this domain, enforced strictly.

- [convention] Playbooks state an objective and numbered steps #delivery
- [convention] Every playbook names its owner #delivery
