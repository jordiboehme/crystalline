---
type: playbook
title: Dependency upgrade playbook
permalink: dependency-upgrade-playbook
tags:
- delivery
- playbook
status: current
recorded_at: 2026-07-07
timestamp: 2026-07-07T05:06:43.676827+00:00
owner: dana
---

How routine upgrades roll through the fleet.

- [objective] Upgrade shared dependencies without breaking consumers
- [step] Upgrade the canary service first
- [step] Watch its error budget for a full day
- [step] Roll the remaining services in dependency order
