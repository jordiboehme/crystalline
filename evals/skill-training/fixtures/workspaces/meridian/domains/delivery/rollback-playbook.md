---
type: playbook
title: Rollback playbook
permalink: rollback-playbook
tags:
- delivery
- playbook
status: current
recorded_at: 2026-07-07
timestamp: 2026-07-07T05:06:43.663591+00:00
owner: miguel
---

How to take a bad release back out.

- [objective] Restore the previous release within ten minutes
- [step] Freeze the deploy pipeline
- [step] Re-promote the previous color
- [step] Confirm error rates return to baseline
