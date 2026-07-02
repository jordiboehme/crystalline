---
type: security
title: Widget Service Security Notes
permalink: import-source/engineering/security-notes
tags: security, widget, review
status: current
recorded_at: 2026-02-15
---

# Widget Service Security Notes

The service authenticates every request with a short lived token issued by
the internal identity provider.

Rotating the signing key requires restarting the daemon so the new key is
picked up before old tokens expire.
