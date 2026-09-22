# From the terminal

The CLI mirrors everything an agent can do; this runs start to finish on a clean machine.

This runs verbatim, start to finish, on a clean machine:

```sh
# 1. Create a domain: a folder of knowledge with a MANIFEST.md at its root.
#    domain add indexes whatever is already there (the manifest, for now)
#    right away, no separate sync step needed.
mkdir -p ~/knowledge/engineering
crystalline domain init ~/knowledge/engineering --name engineering
crystalline domain add engineering ~/knowledge/engineering

# 2. Capture an engram: a unit of knowledge, with an observation bullet.
crystalline write engineering "Retry queue gotcha" \
  --content "- [gotcha] The retry queue drops jobs older than 24h #payments" \
  --tags gotcha,payments

# 3. Search it back (plain text, since no embeddings exist yet).
crystalline search "retry queue"

# 4. Fetch the local embedding model once, then re-sync with embeddings.
crystalline model download
crystalline sync --embed

# 5. Search again: hybrid text-plus-semantic ranking now finds the engram
#    from a differently worded description of the same problem.
crystalline search "why does the payments queue lose jobs"

# 6. See what got indexed.
crystalline status
```

Engrams written through Crystalline are indexed immediately; `crystalline sync` only picks up files created outside it (an editor, a `git pull`) when no daemon is watching them. Edit the domain's `MANIFEST.md` `## Scope` and `## When to Use` sections so routing describes it accurately - that file is what the session prompt and an agent's routing decisions read (see [Session onboarding](../learning-loop.md#session-onboarding)).

The long read is the [Crystalline Handbook](https://jordiboehme.github.io/crystalline/): the whole working loop, chapter by chapter.
